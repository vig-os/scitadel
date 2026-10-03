//! What a paced fetch can fail with.
//!
//! The variants are a report's vocabulary, not an implementation detail
//! (ADR-007 §3, §4). `needs_login`, `rate_limited` and `needs_ill` are
//! different outcomes with different recoveries, so collapsing them is how
//! "retry tomorrow" turns into "this campaign is broken" — the exact failure
//! `PaceDenied` exists to prevent on the pacer side.

use scitadel_core::ports::PaceDenied;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, FetchError>;

#[derive(Debug, Error)]
pub enum FetchError {
    /// A redirect landed on a login or SSO page.
    ///
    /// ADR-007 §4: this records `needs_login` and puts the bucket into
    /// backoff. It is *not* a paywall verdict — scitadel cannot tell, from
    /// a URL, whether a human could satisfy this login with the institution's
    /// credentials.
    #[error("redirected to a login page ({url}) — this work needs a human login")]
    LoginRedirect { url: String },

    /// 429 or 503. The publisher asked us to slow down, and the caller can
    /// put the bucket into backoff using [`FetchError::retry_at_ms`].
    #[error("rate limited by {bucket} (HTTP {code}) — the publisher asked us to slow down")]
    RateLimited {
        /// The platform that misbehaved, so the caller backs off the right
        /// bucket rather than the last URL it happened to touch.
        bucket: String,
        code: u16,
        /// From `Retry-After`, when the publisher sent a delta-seconds value.
        /// Epoch milliseconds. `None` means "no hint, use your own backoff".
        retry_after_ms: Option<i64>,
    },

    /// Any other response that is neither a success nor a redirect.
    #[error("HTTP {code} from {url}")]
    Status { code: u16, url: String },

    /// Connection refused, DNS failure, TLS failure, timeout.
    ///
    /// Carries the URL rather than the whole request, so a credential can
    /// never reach an error string (ADR-007 §5).
    #[error("transport error fetching {url}: {source}")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    /// The pacer refused before anything went on the wire: daily cap,
    /// backoff window, a busy ledger, or a wait too long to block on.
    #[error("pacing refused: {0}")]
    Pace(#[from] PaceDenied),

    /// The redirect budget was spent. The start URL is reported because the
    /// chain that ran away is rarely visible from the last hop.
    #[error("gave up after {hops} redirects starting at {url}")]
    TooManyRedirects { hops: usize, url: String },

    /// A `Location` we could not resolve against the current URL.
    #[error("redirect from {url} has an unusable Location header: {detail}")]
    BadRedirect { url: String, detail: String },

    /// A redirect we will not follow at all.
    #[error("refusing to follow the redirect from {from} to {to}: {reason}")]
    RedirectRefused {
        from: String,
        to: String,
        reason: &'static str,
    },

    /// Refused to *build* a header set carrying a credential the caller did
    /// not ask for — a header set declared unauthenticated is a promise, not
    /// a hint.
    #[error("refusing to carry the {header} header: {detail}")]
    CredentialRefused {
        header: &'static str,
        detail: String,
    },

    /// Refused to *send* a credential to a bucket it is not scoped to.
    ///
    /// ADR-007 §5: "a key is attached only to requests in its own publisher's
    /// bucket, and those requests don't follow redirects to other buckets."
    #[error("refusing to send {header} to {target}: the key is scoped to bucket {allowed}")]
    CredentialOutOfScope {
        header: &'static str,
        target: String,
        allowed: String,
    },

    /// The injected `reqwest::Client` could not be built.
    #[error("could not build the HTTP client: {source}")]
    ClientBuild {
        #[source]
        source: reqwest::Error,
    },
}

impl FetchError {
    /// When the caller should not retry before, in epoch milliseconds, if the
    /// publisher or the ledger told us.
    ///
    /// This is the value a caller feeds to `BackoffControl::set_backoff`.
    pub fn retry_at_ms(&self) -> Option<i64> {
        match self {
            Self::RateLimited { retry_after_ms, .. } => *retry_after_ms,
            Self::Pace(PaceDenied::Backoff { until_ms } | PaceDenied::WaitTooLong { until_ms }) => {
                Some(*until_ms)
            }
            _ => None,
        }
    }

    /// Is this the "come back later" family?
    ///
    /// `PaceDenied::LedgerBusy` is deliberately *not* in it: the ledger being
    /// unavailable is an infrastructure failure, and telling the campaign to
    /// retry later would hide it behind a rate-limit story.
    pub fn is_rate_limited(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Pace(
                    PaceDenied::DailyCap
                        | PaceDenied::Backoff { .. }
                        | PaceDenied::WaitTooLong { .. }
                )
        )
    }

    /// Does this work need a human login (ADR-007 §3's `needs_login`)?
    pub fn needs_login(&self) -> bool {
        matches!(self, Self::LoginRedirect { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every failure has to say something a report can act on. An empty or
    /// unrecognisable message is how a retry loop becomes a silent campaign.
    #[test]
    fn every_variant_explains_itself() {
        let all = [
            FetchError::LoginRedirect {
                url: "https://login.example.org/SignIn".into(),
            },
            FetchError::RateLimited {
                bucket: "elsevier".into(),
                code: 429,
                retry_after_ms: Some(1_700_000_000_000),
            },
            FetchError::Status {
                code: 404,
                url: "https://example.org/pdf".into(),
            },
            FetchError::TooManyRedirects {
                hops: 5,
                url: "https://example.org/".into(),
            },
            FetchError::BadRedirect {
                url: "https://example.org/".into(),
                detail: "not ASCII".into(),
            },
            FetchError::RedirectRefused {
                from: "https://example.org/".into(),
                to: "file:///etc/passwd".into(),
                reason: "only http and https are followed",
            },
            FetchError::CredentialRefused {
                header: "Authorization",
                detail: "the caller asked for an unauthenticated request".into(),
            },
            FetchError::CredentialOutOfScope {
                header: "Cookie",
                target: "springer".into(),
                allowed: "elsevier".into(),
            },
            FetchError::Pace(PaceDenied::DailyCap),
        ];
        for e in all {
            let msg = e.to_string();
            assert!(!msg.trim().is_empty(), "{e:?} has no message");
            assert!(
                msg.len() > 12,
                "{e:?} message is not worth showing a user: {msg}"
            );
        }
    }

    #[test]
    fn rate_limited_and_pacing_denials_are_try_again_later() {
        let rl = FetchError::RateLimited {
            bucket: "elsevier".into(),
            code: 429,
            retry_after_ms: Some(42),
        };
        assert!(rl.is_rate_limited());
        assert_eq!(rl.retry_at_ms(), Some(42));

        for denied in [
            PaceDenied::DailyCap,
            PaceDenied::Backoff { until_ms: 7 },
            PaceDenied::WaitTooLong { until_ms: 9 },
        ] {
            let e = FetchError::Pace(denied);
            assert!(e.is_rate_limited(), "{denied} means try later");
        }
        assert_eq!(
            FetchError::Pace(PaceDenied::Backoff { until_ms: 7 }).retry_at_ms(),
            Some(7)
        );
    }

    /// A busy ledger is an infrastructure failure, not a publisher's answer.
    #[test]
    fn a_busy_ledger_is_not_reported_as_rate_limiting() {
        let e = FetchError::Pace(PaceDenied::LedgerBusy);
        assert!(!e.is_rate_limited(), "LedgerBusy must stay visible");
        assert_eq!(e.retry_at_ms(), None);
    }

    #[test]
    fn needs_login_is_only_the_login_redirect() {
        assert!(
            FetchError::LoginRedirect {
                url: "https://sso.example.org/auth".into()
            }
            .needs_login()
        );
        assert!(
            !FetchError::Status {
                code: 403,
                url: "https://example.org/".into()
            }
            .needs_login()
        );
    }

    #[test]
    fn the_source_chain_reaches_the_pacing_denial() {
        use std::error::Error as _;
        let e = FetchError::Pace(PaceDenied::WaitTooLong { until_ms: 3 });
        assert_eq!(
            e.source().map(ToString::to_string).as_deref(),
            Some("wait would exceed the 10s ceiling (until 3)")
        );
    }
}
