//! Rate limiting across a whole acquisition campaign.
//!
//! The acquisition ladder walks thousands of works across a handful of
//! publisher platforms. Two failure modes matter, and they pull in opposite
//! directions:
//!
//! - Being too fast gets a user's institutional IP suspended. That is
//!   expensive, visible, and not scitadel's to risk.
//! - Being too slow makes a campaign useless. Waiting on one publisher must
//!   not stall work against another.
//!
//! So pacing is **per publisher platform** rather than per host or global,
//! and the ledger is shared across processes through the database — the TUI
//! in one pane and `scitadel mcp` in the next must not each get their own
//! budget. ADR-007 §4.

use async_trait::async_trait;

/// A pacing bucket: one publisher platform, e.g. `elsevier`.
///
/// Several hosts map to one bucket (`sciencedirect.com`,
/// `pdf.sciencedirectassets.com`, `api.elsevier.com` are all `elsevier`),
/// because the limit that matters is the platform's, not the host's.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Bucket(pub String);

impl Bucket {
    /// The publisher-platform name, e.g. `elsevier`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Bucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What kind of request a permit is for. Tiers have separate rolling caps.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PaceTier {
    /// Metadata APIs (OpenAlex, Crossref, DataCite, Europe PMC, Unpaywall).
    Meta,
    /// Open-access repositories and preprint servers.
    Oa,
    /// Sanctioned TDM platforms.
    Tdm,
    /// A human's browser session (tier 4).
    Session,
}

impl PaceTier {
    /// Stable lowercase name, for database values and bucket keys.
    pub fn label(self) -> &'static str {
        match self {
            Self::Meta => "meta",
            Self::Oa => "oa",
            Self::Tdm => "tdm",
            Self::Session => "session",
        }
    }

    /// Every tier, for policy tables and tests that must not miss one.
    pub const ALL: [Self; 4] = [Self::Meta, Self::Oa, Self::Tdm, Self::Session];
}

/// What a permit spends. `Work` is charged once per work per bucket; every
/// hop also spends a `Request`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cost {
    /// One HTTP request (one redirect hop).
    Request,
    /// One work, charged on the first hop into a bucket for that work.
    Work,
}

/// A granted reservation: the caller may not act before `not_before`.
#[derive(Clone, Debug)]
pub struct Permit {
    pub bucket: Bucket,
    pub tier: PaceTier,
    /// When the holder may proceed. Sleep *outside* any database
    /// transaction — see ADR-007 §4 step 5.
    pub not_before: std::time::Instant,
}

/// Why no permit was granted.
///
/// Every variant is a *refusal*, and they are kept distinct because they
/// mean different things to a report: a daily cap and a temporary backoff
/// both yield "try later", but only a cap means the campaign cannot finish
/// today. Collapsing them is how "rate limited" becomes a permanent state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaceDenied {
    /// The rolling 24 h cap for this bucket+tier+unit is spent.
    DailyCap,
    /// The bucket is in a temporary backoff window (a 429 or 503).
    Backoff { until_ms: i64 },
    /// The ledger could not be written. Fails closed: no permit is better
    /// than an unbudgeted request.
    LedgerBusy,
    /// The wait exceeds the 10 s ceiling, so the caller should record
    /// `rate_limited` with `next_attempt_at` and move on to another bucket.
    WaitTooLong { until_ms: i64 },
}

impl std::fmt::Display for PaceDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DailyCap => write!(f, "daily cap reached"),
            Self::Backoff { until_ms } => write!(f, "bucket in backoff until {until_ms}"),
            Self::LedgerBusy => write!(f, "pacing ledger unavailable (failing closed)"),
            Self::WaitTooLong { until_ms } => {
                write!(f, "wait would exceed the 10s ceiling (until {until_ms})")
            }
        }
    }
}

impl std::error::Error for PaceDenied {}

/// Reserves capacity before a request goes out.
///
/// Implementations must be safe to share across processes via a shared
/// database, and must **fail closed**: if the ledger cannot be consulted,
/// return [`PaceDenied::LedgerBusy`] rather than handing out an unbudgeted
/// permit.
#[async_trait]
pub trait Pacer: Send + Sync {
    /// Reserve capacity, or explain why not.
    ///
    /// On success the caller must wait until `permit.not_before` before
    /// acting. A permit is spent when it is *granted*, not when the
    /// resulting fetch succeeds — a failed request still consumed the
    /// publisher's goodwill.
    async fn acquire(
        &self,
        bucket: &Bucket,
        tier: PaceTier,
        cost: Cost,
    ) -> Result<Permit, PaceDenied>;
}

/// Put a bucket into backoff until `until_ms` (epoch milliseconds).
///
/// Used when a publisher answers 429 or 503. The next [`Pacer::acquire`]
/// for that bucket reports [`PaceDenied::Backoff`].
#[async_trait]
pub trait BackoffControl: Send + Sync {
    async fn set_backoff(&self, bucket: &Bucket, until_ms: i64) -> Result<(), PaceDenied>;
    async fn clear_backoff(&self, bucket: &Bucket) -> Result<(), PaceDenied>;
    async fn backoff_until(&self, bucket: &Bucket) -> Result<Option<i64>, PaceDenied>;
}

/// Pacing policy for one bucket class.
///
/// The defaults in ADR-007 §4 are policy choices, not publisher-published
/// limits. Configuration may make any of them **stricter**; there is
/// deliberately no way to loosen one, because a user cannot know what a
/// publisher's actual terms allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BucketPolicy {
    /// Minimum interval between requests, in milliseconds.
    pub min_interval_ms: i64,
    /// Rolling 24 h cap on requests in this tier.
    pub request_cap: i64,
    /// Rolling 24 h cap on works in this tier.
    pub work_cap: i64,
}

impl BucketPolicy {
    pub const fn new(min_interval_ms: i64, request_cap: i64, work_cap: i64) -> Self {
        Self {
            min_interval_ms,
            request_cap,
            work_cap,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy table that forgot a tier would silently leave it unbudgeted.
    #[test]
    fn every_tier_has_a_stable_distinct_label() {
        let mut labels: Vec<&str> = PaceTier::ALL.iter().map(|t| t.label()).collect();
        assert_eq!(labels.len(), PaceTier::ALL.len());
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "tier labels must be distinct");
        for t in PaceTier::ALL {
            assert!(t.label().chars().all(|c| c.is_ascii_lowercase()));
        }
    }

    /// `PaceDenied` variants are a report's vocabulary — a collapsed pair
    /// turns "retry tomorrow" into "this campaign is broken".
    #[test]
    fn every_denial_explains_itself() {
        let all = [
            PaceDenied::DailyCap,
            PaceDenied::Backoff { until_ms: 1 },
            PaceDenied::LedgerBusy,
            PaceDenied::WaitTooLong { until_ms: 2 },
        ];
        for d in all {
            assert!(!d.to_string().is_empty(), "{d:?} needs a message");
        }
        assert!(PaceDenied::DailyCap.to_string().contains("cap"));
        assert!(
            PaceDenied::LedgerBusy.to_string().contains("closed"),
            "failing closed should be visible in the message"
        );
        assert!(
            PaceDenied::WaitTooLong { until_ms: 7 }
                .to_string()
                .contains('7')
        );
    }

    #[test]
    fn bucket_display_is_the_platform_name() {
        assert_eq!(Bucket("elsevier".into()).to_string(), "elsevier");
        assert_eq!(Bucket("elsevier".into()).as_str(), "elsevier");
    }

    /// Policies are data; the constructor exists so a table can be built
    /// without struct-literal noise, and so `const` tables stay possible.
    #[test]
    fn policy_constructor_round_trips() {
        let p = BucketPolicy::new(3000, 1000, 100);
        assert_eq!(p.min_interval_ms, 3000);
        assert_eq!(p.request_cap, 1000);
        assert_eq!(p.work_cap, 100);
    }
}
