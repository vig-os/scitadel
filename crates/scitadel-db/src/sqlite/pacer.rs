//! The shared rate-limit ledger (ADR-007 §4, "The ledger").
//!
//! Pacing that lives in memory gives every process its own budget, and
//! scitadel is routinely two processes against one database file (the TUI in
//! one pane, `scitadel mcp` in the next). Two budgets is twice the traffic a
//! publisher agreed to carry, and an institutional IP suspension is expensive
//! and visible. So the ledger lives in the database and every permit is
//! written down before it is spent.
//!
//! The algorithm is the five numbered steps from the ADR, and each step earns
//! its place:
//!
//! 1. `spawn_blocking`, then `BEGIN IMMEDIATE` — the work is synchronous
//!    SQLite I/O that must not stall the async runtime, and `IMMEDIATE` takes
//!    the write lock *before* the cap is read, so read-then-reserve is atomic
//!    against the other process. A deferred transaction would read the cap,
//!    then upgrade, and lose the race with a `SQLITE_BUSY` at the worst
//!    possible moment.
//! 2. Prune grants older than 24 h — the caps are a rolling window, so the
//!    rows that have aged out are dead weight that would otherwise grow
//!    forever.
//! 3. Deny on backoff or on a spent cap.
//! 4. Otherwise reserve a slot at `max(now, next_allowed_ms)`, insert the
//!    grant, advance `next_allowed_ms`, `COMMIT`.
//! 5. **Sleep outside the transaction** — see [`SqlitePacer::acquire`]; the
//!    wait is the caller's job, and holding a write lock across a multi-second
//!    sleep would wedge every other bucket in the campaign.
//!
//! Two invariants are worth stating outright because everything else follows
//! from them:
//!
//! - **A permit counts when it is granted, not when the fetch succeeds.** A
//!   500 from a publisher still spent the publisher's goodwill, so the row is
//!   written before the request goes out and nothing rolls it back.
//! - **`SQLITE_BUSY` means no permit.** If the ledger cannot be consulted the
//!   answer is [`PaceDenied::LedgerBusy`], never an unbudgeted grant. This is
//!   the one place in scitadel where a failure must *reduce* what it does.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::{Transaction, TransactionBehavior, params};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use scitadel_core::ports::{
    BackoffControl, Bucket, BucketPolicy, Cost, PaceDenied, PaceTier, Pacer, Permit,
};

use crate::error::DbError;
use crate::sqlite::Database;

/// The rolling window a cap is measured over (ADR-007 §4: "rolling 24 h").
const WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// The longest a permit may make its holder wait (ADR-007 §4: "Waits up to
/// 10 s block. Longer waits return `WaitTooLong`").
///
/// A wait longer than this is not a pause, it is a scheduling decision: the
/// caller records `rate_limited` with `next_attempt_at` and the batch
/// scheduler moves on to another bucket, so a work waiting on Elsevier never
/// holds up arXiv.
const WAIT_CEILING_MS: i64 = 10_000;

/// Resolves a bucket to the policy that governs it.
///
/// Deliberately a closure rather than a concrete table type: the mapping
/// host → bucket → policy belongs to `scitadel-http`'s `BucketPolicyTable`,
/// and this crate must not depend on it. A `HashMap<Bucket, BucketPolicy>`
/// adapts with [`SqlitePacer::with_policies`].
pub type PolicyLookup = Arc<dyn Fn(&Bucket) -> BucketPolicy + Send + Sync>;

/// The rolling cap for a (tier, unit) pair **across every bucket**.
///
/// ADR-007 §3 and §4 both require one of these: tier 4 is capped at "≤ 50
/// works per publisher, ≤ 150 works total". The per-publisher half is
/// [`BucketPolicy::work_cap`]; the total has no home there, because
/// `BucketPolicy` is by definition per-bucket. `None` means "no cross-bucket
/// ceiling", which is the default so a caller that does not care is not
/// charged for a lookup on every acquire.
pub type TotalCapLookup = Arc<dyn Fn(PaceTier, Cost) -> Option<i64> + Send + Sync>;

/// SQLite-backed [`Pacer`] and [`BackoffControl`].
///
/// One instance per process. It is cheap to clone-by-`Arc` and safe to share:
/// the in-process gate map is keyed by bucket and created on first use.
pub struct SqlitePacer {
    db: Database,
    policies: PolicyLookup,
    /// Cross-publisher ceiling, consulted inside the transaction after the
    /// per-bucket cap. `None` for the (tier, unit) pair means no ceiling.
    total_caps: Option<TotalCapLookup>,
    /// One `Semaphore(1)` per bucket (ADR-007 §4: "An in-process
    /// `Semaphore(1)` per bucket keeps concurrency at 1").
    ///
    /// This is what keeps `next_allowed_ms` advancing monotonically *within*
    /// a process: two concurrent acquires would otherwise both read the same
    /// cursor and reserve the same slot. It says nothing about fairness, and
    /// it is not a substitute for the database lock — the other process's
    /// acquires are serialised by `BEGIN IMMEDIATE`.
    gates: Mutex<HashMap<Bucket, Arc<Semaphore>>>,
}

impl SqlitePacer {
    /// Build a pacer with an arbitrary policy lookup.
    ///
    /// `policies` is consulted once per acquire, before the transaction, so a
    /// configuration reload can be observed without rebuilding the pacer.
    pub fn new(db: Database, policies: PolicyLookup) -> Self {
        Self {
            db,
            policies,
            total_caps: None,
            gates: Mutex::new(HashMap::new()),
        }
    }

    /// Attach the cross-publisher ceiling required by ADR-007 §3/§4.
    ///
    /// Kept separate from [`Self::new`] because it is a *campaign-wide*
    /// budget rather than a per-publisher policy, and mixing the two is how
    /// "≤ 150 works total" quietly becomes 150 works per publisher.
    #[must_use]
    pub fn with_total_caps(mut self, total_caps: TotalCapLookup) -> Self {
        self.total_caps = Some(total_caps);
        self
    }

    /// Convenience form of [`Self::with_total_caps`] over a plain map.
    #[must_use]
    pub fn with_total_cap_map(self, caps: HashMap<(PaceTier, Cost), i64>) -> Self {
        let map = Arc::new(caps);
        self.with_total_caps(Arc::new(move |tier, cost| map.get(&(tier, cost)).copied()))
    }

    /// Build a pacer from a plain `HashMap`, with `unknown` used for any
    /// bucket the map does not name.
    ///
    /// The fallback is the ADR-007 §4 "unknown host" defaults in production;
    /// passing it explicitly (rather than hard-coding it here) keeps this
    /// crate free of any publisher policy, and lets a test pin a table
    /// without inheriting the built-in defaults.
    pub fn with_policies(
        db: Database,
        policies: HashMap<Bucket, BucketPolicy>,
        unknown: BucketPolicy,
    ) -> Self {
        let table = Arc::new(policies);
        let unknown_cell = Arc::new(unknown);
        Self::new(
            db,
            Arc::new(move |bucket: &Bucket| table.get(bucket).copied().unwrap_or(*unknown_cell)),
        )
    }

    /// The in-process gate for `bucket`, created on first use.
    ///
    /// The lock is released before the caller awaits anything, so no guard is
    /// ever held across an `.await`.
    async fn gate(&self, bucket: &Bucket) -> Arc<Semaphore> {
        let mut gates = self.gates.lock().await;
        Arc::clone(
            gates
                .entry(bucket.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(1))),
        )
    }

    /// Park `bucket` until `until_ms`, recording why.
    ///
    /// The [`BackoffControl`] trait carries no reason, but the column exists
    /// and a 429 and a login redirect are not the same event to whoever reads
    /// the campaign report later, so the reason is an inherent-method
    /// parameter rather than a hard-coded string.
    pub async fn set_backoff_with_reason(
        &self,
        bucket: &Bucket,
        until_ms: i64,
        reason: &str,
    ) -> Result<(), PaceDenied> {
        let db = self.db.clone();
        let bucket = bucket.0.clone();
        let reason = reason.to_string();
        let write = move || -> Result<(), DbError> {
            let conn = db.conn()?;
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
                .map_err(|e| DbError::Migration(format!("failed to take backoff lock: {e}")))?;
            tx.execute(
                "INSERT INTO pacer_buckets (bucket, next_allowed_ms, backoff_until_ms, backoff_reason)
                 VALUES (?1, 0, ?2, ?3)
                 ON CONFLICT(bucket) DO UPDATE SET
                     backoff_until_ms = excluded.backoff_until_ms,
                     backoff_reason   = excluded.backoff_reason",
                params![bucket, until_ms, reason],
            )?;
            tx.commit()
                .map_err(|e| DbError::Migration(format!("failed to commit backoff: {e}")))
        };
        run_blocking(write).await
    }

    /// Read the live backoff deadline, without the `IMMEDIATE` transaction an
    /// [`BackoffControl::backoff_until`] call does not need.
    ///
    /// This is a pure read, so a `SQLITE_BUSY` here means "someone else is
    /// writing", not "the ledger is unusable" — a busy writer commits in
    /// milliseconds and the caller's next poll sees the answer. Returning
    /// `None` rather than an error would silently claim a bucket is not parked
    /// when it is, so the distinction is reported instead.
    async fn read_backoff_until(&self, bucket: &Bucket) -> Result<Option<i64>, PaceDenied> {
        let db = self.db.clone();
        let name = bucket.0.clone();
        let read = move || -> Result<Option<i64>, DbError> {
            let conn = db.conn()?;
            let until: Option<i64> = conn
                .query_row(
                    "SELECT backoff_until_ms FROM pacer_buckets WHERE bucket = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .ok();
            Ok(until.filter(|ms| *ms > now_ms()))
        };
        run_blocking(read).await
    }

    /// How many permits of `tier`/`unit` this bucket has spent in the
    /// rolling window. Exposed for the coverage report.
    pub async fn spent(
        &self,
        bucket: &Bucket,
        tier: PaceTier,
        cost: Cost,
    ) -> Result<i64, PaceDenied> {
        let db = self.db.clone();
        let name = bucket.0.clone();
        let query = move || -> Result<i64, DbError> {
            let conn = db.conn()?;
            let n: i64 = conn.query_row(
                "SELECT count(*) FROM pacer_grants
                 WHERE bucket = ?1 AND tier = ?2 AND unit = ?3 AND granted_at_ms > ?4",
                params![name, tier.label(), unit_label(cost), now_ms() - WINDOW_MS],
                |row| row.get(0),
            )?;
            Ok(n)
        };
        run_blocking(query).await
    }
}

#[async_trait]
impl Pacer for SqlitePacer {
    /// ADR-007 §4 steps 1–5.
    ///
    /// **The sleep happens in the caller, not here.** Step 5 says "sleep
    /// outside the transaction", and the choice is between sleeping here after
    /// the commit and handing back a permit with a future `not_before`. It
    /// goes to the caller because:
    ///
    /// - `PacedClient` already sleeps until `permit.not_before` after every
    ///   acquire, so sleeping here would double the wait.
    /// - The in-process gate is released when this function returns. A caller
    ///   that slept *inside* `acquire` would hold the bucket's slot for the
    ///   duration of the wait, which is the "concurrency at 1" the ADR asks
    ///   for in the wrong place — it would serialise the ledger reservation
    ///   with the network round trip and stall every other work queued behind
    ///   the same bucket.
    /// - The ledger cursor has already advanced and committed, so the wait is
    ///   a pure client-side delay. Holding a write lock across it, or a
    ///   semaphore permit across it, would buy nothing.
    ///
    /// So the transaction ends at `COMMIT` and the returned [`Permit`] carries
    /// the deadline.
    ///
    /// The deadline is the ledger's absolute millisecond value translated into
    /// a monotonic `Instant` through a single [`Clock`] reading taken before the
    /// write. Two properties come from that: a wall-clock adjustment mid-call
    /// cannot shorten the wait or turn it into a negative sleep, and the
    /// translation is faithful to within the microseconds between reading the
    /// wall clock and reading the monotonic one — never early by more than
    /// that.
    async fn acquire(
        &self,
        bucket: &Bucket,
        tier: PaceTier,
        cost: Cost,
    ) -> Result<Permit, PaceDenied> {
        let policy = (self.policies)(bucket);
        let cap = cap_for(policy, cost);
        let gate = self.gate(bucket).await;
        // Concurrency 1 per bucket, held across the transaction only.
        let _serialised: OwnedSemaphorePermit = gate
            .acquire_owned()
            .await
            .map_err(|_| PaceDenied::LedgerBusy)?;

        let db = self.db.clone();
        let name = bucket.0.clone();
        // Resolved before the transaction so the closure is not captured
        // into `spawn_blocking`.
        let total_cap = self.total_caps.as_ref().and_then(|f| f(tier, cost));
        let request = GrantRequest {
            bucket: name,
            tier,
            cost,
            min_interval_ms: policy.min_interval_ms,
            cap,
            total_cap,
        };

        // One reading of both clocks, on this thread, immediately before the
        // write — so the deadline the ledger returns is interpreted from the
        // same instant the ledger reasoned about, and not from a second,
        // slightly later wall-clock reading.
        let clock = Clock::now();
        let decision = run_blocking(move || reserve_slot(&db, &request, clock.ms)).await?;

        match decision {
            GrantDecision::Granted { not_before_ms } => Ok(Permit {
                bucket: bucket.clone(),
                tier,
                // `reserve_slot` only returns `Granted` for a deadline within
                // the 10 s ceiling, so this wait is bounded and `Instant`'s
                // `+` cannot overflow on a hostile `min_interval_ms`.
                not_before: clock.instant + clock.wait_until(not_before_ms),
            }),
            GrantDecision::Denied(denial) => Err(denial),
        }
    }
}

#[async_trait]
impl BackoffControl for SqlitePacer {
    async fn set_backoff(&self, bucket: &Bucket, until_ms: i64) -> Result<(), PaceDenied> {
        self.set_backoff_with_reason(bucket, until_ms, "advisory")
            .await
    }

    async fn clear_backoff(&self, bucket: &Bucket) -> Result<(), PaceDenied> {
        let db = self.db.clone();
        let name = bucket.0.clone();
        let write = move || -> Result<(), DbError> {
            let conn = db.conn()?;
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
                .map_err(|e| DbError::Migration(format!("failed to take clear lock: {e}")))?;
            tx.execute(
                "UPDATE pacer_buckets SET backoff_until_ms = 0, backoff_reason = NULL
                 WHERE bucket = ?1",
                params![name],
            )?;
            tx.commit()
                .map_err(|e| DbError::Migration(format!("failed to commit clear: {e}")))
        };
        run_blocking(write).await
    }

    async fn backoff_until(&self, bucket: &Bucket) -> Result<Option<i64>, PaceDenied> {
        self.read_backoff_until(bucket).await
    }
}

/// One reserve-and-count attempt, reduced to what [`Pacer::acquire`] needs.
struct GrantRequest {
    bucket: String,
    tier: PaceTier,
    cost: Cost,
    min_interval_ms: i64,
    /// The cap for `cost`, already selected from the policy.
    cap: i64,
    /// Cross-publisher cap for (tier, unit), if one is configured.
    total_cap: Option<i64>,
}

/// What the ledger decided. A denial carries no side effect other than the
/// prune (see [`reserve_slot`]).
enum GrantDecision {
    Granted { not_before_ms: i64 },
    Denied(PaceDenied),
}

/// The whole ledger algorithm, synchronous. ADR-007 §4 steps 2–4.
///
/// Runs inside `BEGIN IMMEDIATE`, so the cap read and the grant insert are one
/// atomic step: the other process either is counted or is not, and cannot be
/// counted twice. Every exit path either commits (a grant, or a prune on a
/// denial) or rolls back by dropping the transaction.
fn reserve_slot(
    db: &Database,
    request: &GrantRequest,
    now_ms: i64,
) -> Result<GrantDecision, DbError> {
    let conn = db.conn()?;
    let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
        .map_err(|e| DbError::Migration(format!("pacer failed to take the write lock: {e}")))?;

    // Step 2 — prune. Outside the window a row can never affect a decision,
    // so it is pure garbage collection. It is committed even on a denial:
    // a campaign that sits at `DailyCap` for a day would otherwise keep every
    // row it ever wrote, because its only prune would roll back with it.
    tx.execute(
        "DELETE FROM pacer_grants WHERE granted_at_ms <= ?1",
        params![now_ms - WINDOW_MS],
    )?;

    // A bucket with no row has never been paced, so it starts unbacked-off
    // with a zero cursor.
    tx.execute(
        "INSERT OR IGNORE INTO pacer_buckets (bucket, next_allowed_ms, backoff_until_ms)
         VALUES (?1, 0, 0)",
        params![request.bucket],
    )?;

    let (next_allowed_ms, backoff_until_ms): (i64, i64) = tx.query_row(
        "SELECT next_allowed_ms, backoff_until_ms FROM pacer_buckets WHERE bucket = ?1",
        params![request.bucket],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    // Step 3a — backoff wins over everything, including a fresh window. A
    // parked bucket does not get unbudgeted traffic because the clock rolled.
    if backoff_until_ms > now_ms {
        return commit_as(
            tx,
            GrantDecision::Denied(PaceDenied::Backoff {
                until_ms: backoff_until_ms,
            }),
        );
    }

    // Step 3b — the rolling cap. Scoped to (bucket, tier, unit): the ADR gives
    // each tier and each unit its own budget, and a spent `request_cap` says
    // nothing about `work_cap`.
    let spent: i64 = tx.query_row(
        "SELECT count(*) FROM pacer_grants
         WHERE bucket = ?1 AND tier = ?2 AND unit = ?3 AND granted_at_ms > ?4",
        params![
            request.bucket,
            request.tier.label(),
            unit_label(request.cost),
            now_ms - WINDOW_MS
        ],
        |row| row.get(0),
    )?;
    if spent >= request.cap {
        return commit_as(tx, GrantDecision::Denied(PaceDenied::DailyCap));
    }

    // Step 3c — the cross-publisher ceiling. Counted over *every* bucket for
    // this tier and unit, in the same transaction and against the same index
    // (`idx_pacer_grants_tier`), so it cannot race the per-bucket count the
    // way a separate check would.
    //
    // ADR-007 §3: tier 4 is "≤ 50 works per publisher, ≤ 150 works total".
    // The first half is `request.cap`; without this second half a campaign
    // against 50 publishers would authorise 7,500 works.
    if let Some(total_cap) = request.total_cap {
        let spent_total: i64 = tx.query_row(
            "SELECT count(*) FROM pacer_grants
             WHERE tier = ?1 AND unit = ?2 AND granted_at_ms > ?3",
            params![
                request.tier.label(),
                unit_label(request.cost),
                now_ms - WINDOW_MS
            ],
            |row| row.get(0),
        )?;
        if spent_total >= total_cap {
            return commit_as(tx, GrantDecision::Denied(PaceDenied::DailyCap));
        }
    }

    // Step 4 — reserve the slot. The cursor is per *bucket*, not per unit:
    // both a `Request` and a `Work` grant take a slot from the same
    // `next_allowed_ms`, which is what keeps the bucket at concurrency 1.
    // Charging only `Request` would let a `Work` permit return a deadline the
    // next request would also return, and the minimum interval would stop
    // being enforced after the first hop into a bucket.
    let not_before_ms = now_ms.max(next_allowed_ms);
    let wait_ms = not_before_ms - now_ms;

    // Checked *before* the insert: a refused acquire costs the publisher
    // nothing, so it must not be written to the ledger. Charging it would let
    // a caller burn a cap by asking in a loop and never getting a permit.
    if wait_ms > WAIT_CEILING_MS {
        return commit_as(
            tx,
            GrantDecision::Denied(PaceDenied::WaitTooLong {
                until_ms: not_before_ms,
            }),
        );
    }

    tx.execute(
        "INSERT INTO pacer_grants (bucket, tier, unit, granted_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            request.bucket,
            request.tier.label(),
            unit_label(request.cost),
            now_ms
        ],
    )?;
    tx.execute(
        "UPDATE pacer_buckets SET next_allowed_ms = ?2 WHERE bucket = ?1",
        // Saturating: a pathological `min_interval_ms` must park the bucket
        // (every later acquire is then a `WaitTooLong`) rather than wrap the
        // cursor to a negative instant and hand out a permit immediately.
        params![
            request.bucket,
            not_before_ms.saturating_add(request.min_interval_ms)
        ],
    )?;
    tx.commit()
        .map_err(|e| DbError::Migration(format!("pacer failed to commit a grant: {e}")))?;

    Ok(GrantDecision::Granted { not_before_ms })
}

/// Commit the transaction, then hand back `decision`.
///
/// `commit()` consumes the transaction, so the decision has to be built
/// first. A commit failure is an error, not a silent denial: the caller must
/// not be told "denied" when in fact the ledger may or may not have recorded
/// the prune, and either way it fails closed through [`map_ledger_error`].
fn commit_as(tx: Transaction<'_>, decision: GrantDecision) -> Result<GrantDecision, DbError> {
    tx.commit()
        .map_err(|e| DbError::Migration(format!("pacer failed to commit a denial: {e}")))?;
    Ok(decision)
}

/// Run `job` on the blocking pool and fail closed on any error.
///
/// Every failure mode collapses to [`PaceDenied::LedgerBusy`] — a poisoned
/// `SQLITE_BUSY`, a pool that cannot hand out a connection, a closed join
/// handle. There is no "carry on unbudgeted" branch, by design.
async fn run_blocking<T, F>(job: F) -> Result<T, PaceDenied>
where
    F: FnOnce() -> Result<T, DbError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "pacer blocking task did not complete; failing closed");
            PaceDenied::LedgerBusy
        })?
        .map_err(|e| map_ledger_error(&e))
}

/// True when `err` is lock contention rather than a real defect.
///
/// `SQLITE_BUSY` is the ADR's named case: the other process holds the write
/// lock. `SQLITE_LOCKED` is its in-process sibling — a table is locked by a
/// statement on the same connection — and it is transient for the same
/// reason, so it is grouped with `BUSY` for reporting. Kept separate from
/// [`map_ledger_error`] so a log can say *why* the ledger closed, without ever
/// changing what it returns.
pub(crate) fn is_lock_contention(err: &DbError) -> bool {
    let DbError::Sqlite(rusqlite::Error::SqliteFailure(inner, _)) = err else {
        return false;
    };
    let code = inner.code;
    // `ffi::Error::new` normalises a raw `SQLITE_BUSY` / `SQLITE_LOCKED`
    // (and their `_RECOVERY` / `_SHAREDCACHE` variants, which share the low
    // byte) into these two codes, so matching the enum covers every way
    // SQLite can report contention.
    matches!(
        code,
        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
    )
}

/// The one place a `DbError` becomes a [`PaceDenied`].
///
/// Total by design. The trait has no "internal error" variant, and inventing
/// one would tempt a caller into a fallthrough arm that grants a permit
/// anyway. Losing the *reason* is acceptable — [`is_lock_contention`] keeps it
/// for the log — but losing the fail-closed behaviour is not.
pub(crate) fn map_ledger_error(err: &DbError) -> PaceDenied {
    if is_lock_contention(err) {
        tracing::warn!(error = %err, "pacing ledger was busy; failing closed");
    } else {
        tracing::error!(error = %err, "pacing ledger failed; failing closed");
    }
    PaceDenied::LedgerBusy
}

/// The `pacer_grants.unit` value for a [`Cost`].
fn unit_label(cost: Cost) -> &'static str {
    match cost {
        Cost::Request => "request",
        Cost::Work => "work",
    }
}

/// The rolling cap that governs `cost`.
fn cap_for(policy: BucketPolicy, cost: Cost) -> i64 {
    match cost {
        Cost::Request => policy.request_cap,
        Cost::Work => policy.work_cap,
    }
}

/// One reading of both clocks, taken together.
///
/// The ledger stores absolute epoch milliseconds, so `ms` is what it reasons
/// about. A [`Permit`] carries a monotonic [`std::time::Instant`], so a caller
/// can sleep without subtracting wall-clock values in a loop. Converting
/// between the two is only sound from a single paired reading: `since_epoch`
/// is the full-precision form of `ms`, and `instant` is sampled *after* the
/// wall clock so that `instant + wait` lands late rather than early — a
/// rate limiter that is a few microseconds optimistic is one that over-drives
/// a publisher.
///
/// The residual error between the ledger's millisecond deadline and the
/// returned `Instant` is the sampling gap between the two reads, plus
/// `since_epoch`'s sub-millisecond remainder. It is bounded by well under a
/// millisecond and always in the safe direction.
#[derive(Clone, Copy)]
struct Clock {
    /// Epoch milliseconds, truncated — the ledger's resolution.
    ms: i64,
    /// The same instant at full precision.
    since_epoch: Duration,
    /// The monotonic clock, read second.
    instant: std::time::Instant,
}

impl Clock {
    fn now() -> Self {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let ms = i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX);
        Self {
            ms,
            since_epoch,
            instant: std::time::Instant::now(),
        }
    }

    /// How long to wait for a ledger deadline, from this reading.
    ///
    /// Saturating at zero because a deadline in the past means "no wait", not
    /// a negative sleep.
    fn wait_until(&self, deadline_ms: i64) -> Duration {
        let deadline = u64::try_from(deadline_ms.max(0)).unwrap_or(u64::MAX);
        Duration::from_millis(deadline).saturating_sub(self.since_epoch)
    }
}

/// Epoch milliseconds, saturating at 0 before the epoch.
///
/// A pacer only ever computes `now - WINDOW_MS`, so a pre-epoch clock is
/// harmless — but it must not panic or wrap. The ledger works in truncated
/// milliseconds; [`Clock`] carries the full-precision companion.
fn now_ms() -> i64 {
    Clock::now().ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    fn policy(min_interval_ms: i64, request_cap: i64, work_cap: i64) -> BucketPolicy {
        BucketPolicy::new(min_interval_ms, request_cap, work_cap)
    }

    fn bucket(name: &str) -> Bucket {
        Bucket(name.to_string())
    }

    /// A pacer on a migrated in-memory database, with `policy` governing the
    /// single bucket `name`.
    fn pacer(name: &str, policy: BucketPolicy) -> SqlitePacer {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let mut table = HashMap::new();
        table.insert(bucket(name), policy);
        SqlitePacer::with_policies(db, table, policy)
    }

    fn count_grants(db: &Database, bucket: &str, tier: PaceTier, cost: Cost) -> i64 {
        let conn = db.conn().unwrap();
        conn.query_row(
            "SELECT count(*) FROM pacer_grants
             WHERE bucket = ?1 AND tier = ?2 AND unit = ?3",
            params![bucket, tier.label(), unit_label(cost)],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// Unwrap a refusal. `Permit` has no `PartialEq` — it carries an `Instant` —
    /// so denials are compared directly rather than as a whole `Result`.
    fn denial(result: Result<Permit, PaceDenied>) -> PaceDenied {
        result.expect_err("expected a denial, got a permit")
    }

    /// ADR-007 §3: tier 4 is "≤ 50 works per publisher, ≤ 150 works total".
    /// The per-publisher half is the bucket policy; this pins the half that
    /// has no home there. Without it, four publishers at 50 works each would
    /// authorise 200 — the ceiling would silently be per-publisher, which is
    /// the failure mode the cross-bucket count exists to prevent.
    #[tokio::test]
    async fn the_cross_publisher_ceiling_binds_across_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("p.db")).unwrap();
        db.migrate().unwrap();

        // Generous per-publisher caps, so only the total can be the reason
        // for a denial. If this test passed because of `request_cap` it would
        // be testing the wrong thing.
        let generous = BucketPolicy::new(0, 100, 100);
        let pacer = SqlitePacer::with_policies(db.clone(), HashMap::new(), generous)
            .with_total_cap_map(HashMap::from([((PaceTier::Session, Cost::Work), 3)]));

        // Three different publishers, one work each: all allowed.
        for publisher in ["elsevier", "wiley", "springer"] {
            let permit = pacer
                .acquire(&Bucket(publisher.into()), PaceTier::Session, Cost::Work)
                .await
                .unwrap_or_else(|e| panic!("{publisher} work 1: {e}"));
            assert_eq!(permit.bucket.as_str(), publisher);
        }

        // A fourth publisher is over the ceiling, and its own budget is
        // untouched — so the denial can only be the total.
        let err = pacer
            .acquire(&Bucket("acm".into()), PaceTier::Session, Cost::Work)
            .await
            .expect_err("the 4th work across publishers must exceed the ceiling of 3");
        assert_eq!(err, PaceDenied::DailyCap);
        assert_eq!(
            pacer
                .spent(&Bucket("acm".into()), PaceTier::Session, Cost::Work)
                .await
                .unwrap(),
            0,
            "a denied grant must not be charged to the bucket"
        );

        // A different tier is unaffected: the ceiling is per (tier, unit).
        assert!(
            pacer
                .acquire(&Bucket("acm".into()), PaceTier::Oa, Cost::Work)
                .await
                .is_ok(),
            "the ceiling is scoped to tier 4 works, not to works at large"
        );
    }

    /// The ceiling must not fire when no total is configured — the default
    /// has to be "unbounded", or adding the check would have silently capped
    /// every campaign at zero.
    #[tokio::test]
    async fn no_configured_ceiling_means_no_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("p.db")).unwrap();
        db.migrate().unwrap();

        let pacer = SqlitePacer::with_policies(db, HashMap::new(), BucketPolicy::new(0, 1, 1));

        // `request_cap` is 1, but these are Work grants against the
        // per-bucket `work_cap` of 1 on four distinct buckets, so only a
        // wrongly-invented global cap could deny them.
        for publisher in ["a", "b", "c", "d"] {
            assert!(
                pacer
                    .acquire(&Bucket(publisher.into()), PaceTier::Session, Cost::Work)
                    .await
                    .is_ok(),
                "{publisher} must not be denied by an unconfigured ceiling"
            );
        }
    }

    /// A `Duration` for a millisecond count. Policies are `i64` because the
    /// database columns are; `Instant` arithmetic wants `u64`.
    fn ms(millis: i64) -> Duration {
        Duration::from_millis(millis.max(0) as u64)
    }

    /// `pacer_buckets.next_allowed_ms` — the shared cursor, in ledger time.
    fn cursor(db: &Database, bucket: &str) -> i64 {
        db.conn()
            .unwrap()
            .query_row(
                "SELECT next_allowed_ms FROM pacer_buckets WHERE bucket = ?1",
                params![bucket],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// Tolerance when comparing two projected `Instant` deadlines.
    ///
    /// `next_allowed_ms` is an INTEGER of epoch milliseconds — that is the
    /// schema, shared across processes — so the authoritative spacing is exact
    /// and is asserted against the cursor itself, without slack. A deadline
    /// reaches the caller as an `Instant` translated through one paired
    /// wall-clock/monotonic reading per acquire, and the two reads can be
    /// separated by a scheduler preemption. Five milliseconds on an interval
    /// measured in thousands is the honest place for that: wide enough not to
    /// be a coin flip on a loaded CI box, narrow enough that losing it would
    /// mean the cursor is not advancing.
    const PROJECTION_SLACK: Duration = Duration::from_millis(5);

    /// ADR-007 §4: the minimum interval is the reason the cursor exists. Two
    /// sequential acquires must return increasing deadlines, the shared ledger
    /// must move forward by a full interval between them, and the deadline the
    /// caller gets must match the ledger's absolute value.
    #[tokio::test]
    async fn min_interval_is_enforced() {
        let interval = 3_000;
        let pacer = pacer("elsevier", policy(interval, 100, 100));
        let b = bucket("elsevier");

        let first = pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .unwrap();
        let cursor_after_first = cursor(&pacer.db, "elsevier");

        let second = pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .unwrap();
        let cursor_after_second = cursor(&pacer.db, "elsevier");

        // 1. The cross-process guarantee, exact: the cursor advances by a full
        //    minimum interval per grant, whatever the clock does in between.
        assert!(
            cursor_after_second - cursor_after_first >= interval,
            "the cursor advanced by {}ms, min interval is {interval}ms",
            cursor_after_second - cursor_after_first
        );

        // 2. Increasing deadlines, so a caller cannot mistake two permits for
        //    one slot.
        assert!(
            second.not_before > first.not_before,
            "a zero-gap reservation would let one cursor slot serve two requests"
        );

        // 3. At least an interval apart, allowing the paired-clock read.
        let gap = second
            .not_before
            .saturating_duration_since(first.not_before);
        assert!(
            gap + PROJECTION_SLACK >= ms(interval),
            "the permits are only {gap:?} apart, min interval is {interval}ms"
        );

        // 4. And the deadline tracks the ledger's absolute value: the wait is
        //    measured from before the transaction, so the returned `Instant`
        //    can be *later* than the same deadline projected from here, but
        //    never earlier. This is the exact, tolerance-free check that the
        //    `Instant` a caller sleeps to is the ledger's deadline.
        let after = Clock::now();
        let ledger_deadline = after.instant + ms(cursor_after_second - after.ms);
        assert!(
            second.not_before <= ledger_deadline,
            "the permit lands at {:?}, later than the ledger deadline {:?}",
            second.not_before,
            ledger_deadline
        );
    }

    /// The cap is the thing that keeps an institution out of a suspension
    /// notice, and `cap` permits is exactly `cap` permits.
    #[tokio::test]
    async fn a_cap_denies_after_the_cap_is_spent() {
        let pacer = pacer("openalex", policy(0, 3, 100));
        let b = bucket("openalex");

        for i in 0..3 {
            pacer
                .acquire(&b, PaceTier::Meta, Cost::Request)
                .await
                .unwrap_or_else(|e| panic!("permit {i} of 3 should have been granted, got {e}"));
        }
        assert_eq!(
            denial(pacer.acquire(&b, PaceTier::Meta, Cost::Request).await),
            PaceDenied::DailyCap
        );

        // The refusal is not charged: 3 granted, not 4 recorded.
        assert_eq!(
            count_grants(&pacer.db, "openalex", PaceTier::Meta, Cost::Request),
            3
        );
    }

    /// `request_cap` and `work_cap` are separate budgets. A bucket that has
    /// spent every request may still be entitled to the works it is midway
    /// through fetching — refusing the `Work` grant there would strand work
    /// that already has an artefact in flight.
    #[tokio::test]
    async fn work_and_request_caps_are_tracked_separately() {
        let pacer = pacer("arxiv", policy(0, 2, 10));
        let b = bucket("arxiv");

        for _ in 0..2 {
            pacer
                .acquire(&b, PaceTier::Oa, Cost::Request)
                .await
                .expect("request budget");
        }
        assert_eq!(
            denial(pacer.acquire(&b, PaceTier::Oa, Cost::Request).await),
            PaceDenied::DailyCap,
            "the request budget is spent"
        );

        pacer
            .acquire(&b, PaceTier::Oa, Cost::Work)
            .await
            .expect("a spent request budget must not deny a work grant");
        assert_eq!(
            count_grants(&pacer.db, "arxiv", PaceTier::Oa, Cost::Work),
            1
        );
    }

    /// Backoff is the ledger's answer to a 429 or a login redirect: refuse
    /// everything until the deadline, then behave as if nothing happened.
    #[tokio::test]
    async fn backoff_denies_until_cleared() {
        let pacer = pacer("elsevier", policy(0, 100, 100));
        let b = bucket("elsevier");
        let until_ms = now_ms() + 60_000;

        pacer.set_backoff(&b, until_ms).await.unwrap();
        assert_eq!(
            denial(pacer.acquire(&b, PaceTier::Tdm, Cost::Request).await),
            PaceDenied::Backoff { until_ms }
        );
        assert_eq!(pacer.backoff_until(&b).await.unwrap(), Some(until_ms));
        // Nothing was granted while parked.
        assert_eq!(
            count_grants(&pacer.db, "elsevier", PaceTier::Tdm, Cost::Request),
            0
        );

        pacer.clear_backoff(&b).await.unwrap();
        assert_eq!(pacer.backoff_until(&b).await.unwrap(), None);
        pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .expect("a cleared bucket must serve again");
    }

    /// An expired deadline is not a backoff. Otherwise a bucket parked last
    /// Tuesday would be parked for ever.
    #[tokio::test]
    async fn an_expired_backoff_does_not_deny() {
        let pacer = pacer("springer", policy(0, 10, 10));
        let b = bucket("springer");

        pacer.set_backoff(&b, now_ms() - 1).await.unwrap();
        assert_eq!(pacer.backoff_until(&b).await.unwrap(), None);
        pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .expect("a lapsed backoff must not deny");
    }

    /// ADR-007 §4: "Waits up to 10 s block. Longer waits return
    /// `WaitTooLong`." A refusal past the ceiling is what becomes
    /// `rate_limited` with a `next_attempt_at`, so it must be distinguishable
    /// from a spent cap — the cap says "this campaign cannot finish today",
    /// the ceiling says "try this bucket again after lunch".
    #[tokio::test]
    async fn a_wait_beyond_ten_seconds_is_refused() {
        let pacer = pacer("wiley", policy(15_000, 100, 100));
        let b = bucket("wiley");

        // The first permit is available immediately, so a long interval is
        // only visible from the second onward.
        pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .expect("the first permit has no wait");

        let refusal = pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .expect_err("a 15s wait exceeds the 10s ceiling");
        let PaceDenied::WaitTooLong { until_ms } = refusal else {
            panic!("expected WaitTooLong, got {refusal}");
        };
        assert!(
            until_ms > now_ms() + 10_000,
            "the refusal must carry a real deadline, got {until_ms}"
        );
        // And a refused acquire is not written to the ledger.
        assert_eq!(
            count_grants(&pacer.db, "wiley", PaceTier::Tdm, Cost::Request),
            1
        );
    }

    /// A wait of exactly the ceiling is still allowed to block, per the ADR's
    /// "up to 10 s block".
    #[tokio::test]
    async fn a_wait_of_exactly_the_ceiling_blocks() {
        let pacer = pacer("sage", policy(10_000, 100, 100));
        let b = bucket("sage");

        pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .unwrap();
        pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .expect("exactly 10s is within the ceiling");
    }

    /// ADR-007 §4, the reason this whole implementation exists: "the TUI in
    /// one pane and `scitadel mcp` in the next must not each get their own
    /// budget."
    ///
    /// Two `Database` handles on one file is the in-process stand-in for two
    /// processes: separate pools, separate connections, no shared memory. Each
    /// side runs on its own OS thread with its own runtime, releases a barrier
    /// so the two threads enter `acquire` at the same instant, and both try to
    /// spend a budget of four. Exactly four permits may exist, no more.
    ///
    /// A read-then-write ledger that did not take the write lock before
    /// reading the cap would let both sides count 3, both insert, and hand
    /// out 8 permits against a cap of 4.
    #[test]
    fn two_processes_do_not_exceed_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pacer.db");

        let cap = 4;
        let per_side = 6;
        let table = {
            let mut t = HashMap::new();
            t.insert(bucket("elsevier"), policy(0, cap, cap));
            Arc::new(t)
        };

        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|side| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                let table = Arc::clone(&table);
                std::thread::spawn(move || {
                    // Each side opens and migrates the same file for itself,
                    // exactly as two processes starting together would.
                    let db = Database::open(&path).unwrap();
                    db.migrate().unwrap();
                    let pacer = SqlitePacer::with_policies(
                        db.clone(),
                        (*table).clone(),
                        policy(0, cap, cap),
                    );

                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let b = bucket("elsevier");
                    barrier.wait();

                    let mut granted = 0usize;
                    for i in 0..per_side {
                        match runtime.block_on(pacer.acquire(&b, PaceTier::Tdm, Cost::Request)) {
                            Ok(_) => granted += 1,
                            Err(PaceDenied::DailyCap) => {}
                            Err(other) => panic!("side {side}, attempt {i}: unexpected {other}"),
                        }
                    }
                    (side, granted, db)
                })
            })
            .collect();

        let mut granted_total = 0usize;
        let mut last_db = None;
        for handle in handles {
            let (_side, granted, db) = handle.join().unwrap();
            granted_total += granted;
            last_db = Some(db);
        }

        assert_eq!(
            granted_total, cap as usize,
            "exactly {cap} permits may exist across both processes"
        );

        // And the ledger agrees, read from outside both writers.
        let db = last_db.unwrap();
        let conn = db.conn().unwrap();
        let rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM pacer_grants WHERE bucket = ?1 AND tier = ?2 AND unit = ?3",
                params!["elsevier", PaceTier::Tdm.label(), "request"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            rows, cap,
            "the shared ledger must hold exactly {cap} grant rows, not {rows}"
        );
    }

    /// Step 2 is not housekeeping for its own sake: a campaign that runs for
    /// weeks must not accumulate every permit it ever took, and a stale row
    /// inside the window would keep counting against the cap.
    #[tokio::test]
    async fn grants_are_pruned_after_the_window() {
        let pacer = pacer("epo", policy(0, 10, 10));
        let b = bucket("epo");

        // Two rows: one inside the window, one well outside it, inserted
        // directly so the test does not depend on a 24 h wait.
        {
            let conn = pacer.db.conn().unwrap();
            for age in [1_000i64, WINDOW_MS + 60_000] {
                conn.execute(
                    "INSERT INTO pacer_grants (bucket, tier, unit, granted_at_ms)
                     VALUES (?1, ?2, ?3, ?4)",
                    params!["epo", "meta", "request", now_ms() - age],
                )
                .unwrap();
            }
            let before: i64 = conn
                .query_row("SELECT count(*) FROM pacer_grants", [], |row| row.get(0))
                .unwrap();
            assert_eq!(before, 2);
        }

        pacer
            .acquire(&b, PaceTier::Meta, Cost::Request)
            .await
            .expect("an in-window grant plus a fresh one are within the cap of 10");

        let conn = pacer.db.conn().unwrap();
        let aged: i64 = conn
            .query_row(
                "SELECT count(*) FROM pacer_grants WHERE granted_at_ms <= ?1",
                params![now_ms() - WINDOW_MS],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(aged, 0, "the grant outside the 24 h window must be gone");
        let kept: i64 = conn
            .query_row("SELECT count(*) FROM pacer_grants", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, 2, "the in-window grant and the new one must survive");
    }

    /// The ADR's fail-closed rule, pinned at the mapping so no future
    /// refactor can quietly add a fallthrough that grants anyway.
    #[test]
    fn ledger_busy_fails_closed() {
        let busy = DbError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".to_string()),
        ));
        assert!(is_lock_contention(&busy), "SQLITE_BUSY is contention");
        assert_eq!(map_ledger_error(&busy), PaceDenied::LedgerBusy);

        let locked = DbError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED),
            None,
        ));
        assert!(is_lock_contention(&locked), "SQLITE_LOCKED is contention");

        // Every other failure closes the ledger too. A malformed schema must
        // not become a permit, so `map_ledger_error` is total — and is
        // therefore pinned here over the `DbError` variants a caller can
        // actually construct. (`DbError::Pool` is absent because
        // `r2d2::Error` is an opaque tuple struct with no public
        // constructor; its totality is guaranteed by `map_ledger_error`'s
        // signature rather than by an instance.)
        let others = [
            DbError::Sqlite(rusqlite::Error::InvalidQuery),
            DbError::Sqlite(rusqlite::Error::QueryReturnedNoRows),
            DbError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                None,
            )),
            DbError::Migration("no such table: pacer_grants".into()),
            DbError::Migration("pacer failed to take the write lock".into()),
        ];
        for err in &others {
            assert!(
                !is_lock_contention(err),
                "{err} is a defect, not contention — the log must say so"
            );
            assert_eq!(
                map_ledger_error(err),
                PaceDenied::LedgerBusy,
                "{err} must still fail closed"
            );
        }
    }

    /// The gate is per bucket, so a work waiting on Elsevier must not hold up
    /// arXiv (ADR-007 §4). Two buckets, acquired concurrently, both proceed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn gates_are_per_bucket() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let mut table = HashMap::new();
        table.insert(bucket("elsevier"), policy(0, 10, 10));
        table.insert(bucket("arxiv"), policy(0, 10, 10));
        let pacer = Arc::new(SqlitePacer::with_policies(db, table, policy(0, 10, 10)));

        let first = {
            let pacer = Arc::clone(&pacer);
            tokio::spawn(async move {
                pacer
                    .acquire(&bucket("elsevier"), PaceTier::Tdm, Cost::Request)
                    .await
            })
        };
        let second = {
            let pacer = Arc::clone(&pacer);
            tokio::spawn(async move {
                pacer
                    .acquire(&bucket("arxiv"), PaceTier::Oa, Cost::Request)
                    .await
            })
        };

        first.await.unwrap().expect("elsevier permit");
        second.await.unwrap().expect("arxiv permit");
    }

    /// A missing policy must not silently unbudget a bucket. `with_policies`
    /// falls back rather than defaulting to zero caps (which would deny
    /// everything) or `i64::MAX` (which would allow everything); the fallback
    /// is the caller's ADR-007 §4 "unknown host" defaults.
    #[tokio::test]
    async fn an_unlisted_bucket_uses_the_fallback_policy() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let pacer = SqlitePacer::with_policies(db, HashMap::new(), policy(0, 2, 2));

        let b = bucket("unknown.example");
        pacer
            .acquire(&b, PaceTier::Oa, Cost::Request)
            .await
            .unwrap();
        pacer
            .acquire(&b, PaceTier::Oa, Cost::Request)
            .await
            .unwrap();
        assert_eq!(
            denial(pacer.acquire(&b, PaceTier::Oa, Cost::Request).await),
            PaceDenied::DailyCap,
            "the fallback policy's cap governs an unlisted bucket"
        );
    }

    /// A `Work` grant takes a slot from the same per-bucket cursor a `Request`
    /// does, and that is what keeps the minimum interval honest across a
    /// multi-hop fetch.
    ///
    /// With `min_interval_ms = 2_000` and the `Request`-then-`Work` pair
    /// `PacedClient` takes on the first hop into a bucket, the arithmetic is:
    ///
    /// ```text
    /// t = 0        Request -> not_before = t,        cursor = t + 2s
    /// t = 0        Work    -> not_before = t + 2s,  cursor = t + 4s
    /// t = 2s       Request -> not_before = t + 4s,  cursor = t + 6s
    /// ```
    ///
    /// The first hop of a new work therefore waits one interval, and the
    /// requests still leave 2 s apart. Had `Work` been granted a pass on the
    /// cursor, the second request would have returned `t + 2s` as well and two
    /// requests would have gone out back to back.
    #[tokio::test]
    async fn a_work_grant_also_advances_the_bucket_cursor() {
        let interval = 2_000;
        let pacer = pacer("elsevier", policy(interval, 100, 100));
        let b = bucket("elsevier");

        let request = pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .unwrap();
        let work = pacer.acquire(&b, PaceTier::Tdm, Cost::Work).await.unwrap();
        assert!(
            work.not_before >= request.not_before,
            "a Work grant can never be dated before the Request it followed"
        );

        let next = pacer
            .acquire(&b, PaceTier::Tdm, Cost::Request)
            .await
            .unwrap();
        let gap = next.not_before.saturating_duration_since(work.not_before);
        assert!(
            gap + PROJECTION_SLACK >= ms(interval),
            "the next request must still wait a full interval, waited {gap:?}"
        );
        let whole = next
            .not_before
            .saturating_duration_since(request.not_before);
        assert!(
            whole + PROJECTION_SLACK >= ms(2 * interval),
            "Request -> Work -> Request must span two intervals, spanned {whole:?}"
        );
    }
}
