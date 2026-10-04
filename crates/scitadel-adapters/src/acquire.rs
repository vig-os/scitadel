//! `acquire` — the queue runner (ADR-007 §2 "Status vocabulary", §3 "Routes
//! and the ladder").
//!
//! # The acceptance criterion, and where it is decided
//!
//! > A re-run makes no network calls for artefacts already held.
//!
//! That is not "the second run leaves the same rows afterwards". It is *the
//! second run must not even ask a publisher*. So the decision is made before
//! a single byte moves, from the derived "have":
//!
//! - [`scitadel_db::sqlite::coverage_report`] walks `acquisition_state`
//!   against `artefacts` and returns only the wants we do **not** hold
//!   ([`scitadel_db::sqlite::CoverageReport::entries`]). A work whose want is
//!   satisfied is not in that list, so [`plan`] never puts it in
//!   `plan.works` and [`run`] never fetches it.
//! - There is deliberately **no second derivation of "have"** in this file.
//!   `is_satisfied`, `version_satisfies` and the `missing_on_disk = 0` clause
//!   are `coverage.rs`'s, and #260/#261/#275 are three incidents caused by a
//!   second copy of one of those rules existing somewhere.
//!
//! Two independent things close a gap, and the same derivation has to cover
//! both rather than one of them by luck:
//!
//! - a successful fetch **deletes** the want row (`record_download`, in the
//!   same transaction as the artefact), so the work leaves the queue by
//!   itself;
//! - a want row owned by a *different* writer — the flat importer's, which
//!   carries a `drop_path` — survives a download. It must leave the queue
//!   because the derivation says it is satisfied, not because the row
//!   vanished. `a_second_run_makes_no_network_calls_for_held_artefacts` pins
//!   both shapes.
//!
//! # What this module writes, and what it refuses to
//!
//! Three writers touch `acquisition_state` now, and each owns its own columns:
//!
//! | writer | status | `publisher` | `next_attempt_at` |
//! |---|---|---|---|
//! | flat importer | `pending` | `None` (no DOI to classify from) | `None` |
//! | ladder ([`crate::download`]) | what the legs established | `None` (no route evaluated) | `None` |
//! | `acquire` | `pending`, and only on a row it creates | when it created the row | after an `error` |
//!
//! `acquire` changes **no status that a walk established**. It reports the
//! status the walk wrote, read back off the row ([`WorkOutcome::status`]), and
//! it adds the one thing it is itself the authority for: the retry time.
//! ADR-007 §2 routes `error` to "retried with backoff by `acquire`", so that
//! sentence is this module's to keep.
//!
//! # Where `acquire` deliberately does not write
//!
//! - **`rate_limited`.** ADR-007 §4 assigns that write to whoever reads the
//!   bucket ledger — "longer waits return `WaitTooLong`, which becomes
//!   `rate_limited` with `next_attempt_at`". `download_paper` surfaces its
//!   leg failures as an [`AdapterError`], so `FetchError::is_rate_limited`
//!   never reaches this layer and nothing here *established* a bucket-level
//!   refusal. `acquire --resume` **reads and honours** such a row; the
//!   campaign scheduler that writes one lands with ADR-007 §4's batch
//!   round-robin. Writing it from a walk's error string would be the #260
//!   defect in a new place — a work-level row carrying a bucket-level claim.
//! - **`unavailable`.** Only a walk in which every *evaluated* leg was refused
//!   by the place itself establishes it, and that derivation is
//!   `download::Ladder`'s. An index that names no location is `error`;
//!   `an_index_naming_no_location_never_yields_unavailable` pins that through
//!   `acquire` too, because a queue runner that reported a work as
//!   unavailable would be a third writer of a verdict it never reached.
//! - **`acquisition_state.publisher_note`.** That column exists on
//!   `artefacts` (migration 014) and **not** on `acquisition_state`, so there
//!   is nowhere on a gap row to put [`RouteVerdict::PublisherUnknown`]'s
//!   wording. Rather than invent a migration here, the note travels on the
//!   report ([`PlannedWork::publisher_note`]) and `action_list` prints it from
//!   the registry at read time.
//!
//! # `--dry-run`
//!
//! The plan is pure reads, so a dry run *is* the plan and nothing else: no
//! request, no write, and an empty `outcomes` list.
//! `dry_run_writes_nothing_and_calls_nothing` compares the whole of
//! `acquisition_state`, `artefacts` and the legacy `papers` columns across a
//! dry run and counts wiremock requests.
//!
//! # Where this lives
//!
//! In `scitadel-adapters`, beside the ladder it drives, rather than in the
//! `scitadel-acquire` crate ADR-007 §3 wants. Splitting a runner with no
//! callers outside these two crates across two crates would invent a
//! dependency edge for nothing; moving the module later is mechanical.

use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use scitadel_core::models::{Paper, validate_doi};
use scitadel_core::ports::PaperRepository as _;
use scitadel_db::error::DbError;
use scitadel_db::sqlite::{
    CoverageError, Database, FULLTEXT_LOCATOR, MissingEntry, StateRow, StateWrite, status_vocab,
    write_acquisition_states,
};
use serde::Serialize;

use crate::download::PaperDownloader;
use crate::error::AdapterError;
use crate::manifest;

/// ADR-007, #253's last acceptance criterion: "raid's `p1`/`p3` NDJSON
/// acquisition fields import via `acquire --from works.ndjson` (curation statuses
/// stay raid's)". The importer lives beside this module and is re-exported here,
/// because the queue import is part of *acquire* rather than a second way into
/// `acquisition_state` — an imported gap must become actionable by
/// [`plan`], which is only true if it arrived through the same table this
/// module reads.
pub use crate::acquire_ndjson::{
    IdentityDisagreement, NdjsonImport, PreservedStatus, import_ndjson_queue, stored_status,
};

/// The one `acquisition_state.kind` this ladder can close.
///
/// ADR-007 §3's six routes are full-text routes — every one ends in "the
/// article" — so a `fulltext` want is what a fetch can satisfy and nothing
/// else is.
pub const FULLTEXT_KIND: &str = "fulltext";

/// The version a full-text want asks for.
///
/// The version of record, matching ADR-007 §2's column default, the ladder's
/// gap and the flat importer's. A preprint we happen to find is a held
/// artefact at `version = 'preprint'`, not the want being satisfied.
const WANTED_VERSION: &str = "vor";

/// `acquisition_state.status` for a queued work nobody has tried yet.
const STATUS_PENDING: &str = "pending";

/// How long a work's lease is held for while its ladder walk runs. ADR-007 §1's
/// own default; see [`scitadel_db::sqlite::leases`].
pub const LEASE_TTL_MS: i64 = scitadel_db::sqlite::LEASE_TTL_MS;

/// How long a transient failure waits before `acquire --resume` retries it.
///
/// A **policy choice, not a publisher-published limit** — the same status
/// ADR-007 §4 gives its defaults, for the same reason: there is no setting
/// that makes it shorter, because a shorter wait is the direction that costs a
/// publisher goodwill.
///
/// Anchored to *when the failure was established* rather than to the clock at
/// write time (see [`schedule_retry`]), which is what makes a re-run over an
/// unchanged failure write nothing at all.
pub const RETRY_BACKOFF_MINUTES: i64 = 15;

/// The `acquisition_state.status` values `acquire` will fetch for, taken from
/// ADR-007 §2's own routing:
///
/// - `pending` — "picked up again by `acquire`";
/// - `error` — "retried with backoff by `acquire`";
/// - `rate_limited` — "retried at `next_attempt_at` by `acquire`";
/// - `oa_fetchable` — "an OA location is known, not yet fetched";
/// - `tdm_available` — "a sanctioned TDM route exists and credentials are
///   present".
///
/// The last two are unreachable today — nothing in this slice persists a
/// located OA PDF or a TDM credential, which is ADR-007 §4's route table's
/// job — and they are listed anyway so the writer that does land does not have
/// to edit the queue to be honoured.
///
/// Deliberately **not** the rest of §2's "—" column:
///
/// - `unavailable` means the work has no such artefact, and §2 says "no
///   action" — re-asking a publisher about it is the wrong behaviour, not a
///   cautious one;
/// - every status with a human action in §2 (`tdm_key_missing`,
///   `needs_login`, `not_entitled`, `needs_authorisation`, `needs_ill`,
///   `identity_mismatch`, `wrong_version`) belongs to `action_list`. Fetching
///   a `needs_ill` row because it shares a table with the queue is how a
///   machine ends up re-asking about a work whose obstacle is an ILL request.
pub const ACQUIRE_QUEUE_STATUSES: [&str; 5] = [
    "pending",
    "error",
    "rate_limited",
    "oa_fetchable",
    "tdm_available",
];

/// What to acquire.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcquireRequest {
    /// Restrict the run to these works (full id or unambiguous prefix).
    /// Empty means the whole queue.
    ///
    /// An explicit id bypasses the [`ACQUIRE_QUEUE_STATUSES`] filter — the
    /// caller named this work — but not `--resume`'s deferral, and not the
    /// derivation: a work with no *missing* want is still not fetched, and the
    /// report says so in [`AcquirePlan::unmatched`].
    pub paper_ids: Vec<String>,
    /// One `acquisition_state.kind`; only `"fulltext"` is acquirable.
    pub kind: Option<String>,
    /// Stop after this many works. Applied **after** the deferral filter, so
    /// `--limit` never spends the budget on a work that is not due.
    pub limit: Option<usize>,
    /// Only fetch what is due: defer any gap whose `next_attempt_at` is in the
    /// future (ADR-007 §2's `rate_limited`).
    pub resume: bool,
    /// Plan only: no writes, no network.
    pub dry_run: bool,
}

impl AcquireRequest {
    /// A plain drain of the whole `fulltext` queue.
    #[must_use]
    pub fn queue() -> Self {
        Self::default()
    }

    /// With `paper_ids` set — the MCP shape, `acquire(paper_ids, …)`.
    #[must_use]
    pub fn for_papers<I>(ids: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        Self {
            paper_ids: ids.into_iter().collect(),
            ..Self::default()
        }
    }

    /// `resume` on.
    #[must_use]
    pub fn resuming(mut self) -> Self {
        self.resume = true;
        self
    }

    /// `dry_run` on.
    #[must_use]
    pub fn planning(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// `limit` set.
    #[must_use]
    pub fn limited(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// `kind` set.
    #[must_use]
    pub fn of_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// The kind this request acquires, resolved.
    #[must_use]
    pub fn resolved_kind(&self) -> &str {
        self.kind.as_deref().unwrap_or(FULLTEXT_KIND)
    }
}

/// One work `acquire` decided to fetch, and everything it knows about it
/// **before** it fetched anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedWork {
    pub paper_id: String,
    pub kind: String,
    pub locator: String,
    pub wanted_version: String,
    /// Why this gap is queued: the row's `acquisition_state.status`.
    pub status: String,
    pub next_attempt_at: Option<String>,
    /// A publisher the DOI registry can name, or `None`.
    ///
    /// Derived through [`PublisherKey::of`], never guessed (#261) and never
    /// read back from the stored column: a name this build cannot derive is a
    /// name it cannot check.
    pub publisher: Option<String>,
    /// [`RouteVerdict::PublisherUnknown`]'s note, verbatim, when the prefix is
    /// not in the registry. "no route was evaluated" — never "no TDM route
    /// available", because nothing evaluated one.
    pub publisher_note: Option<String>,
    pub doi: Option<String>,
}

/// A work the plan left out under `--resume`, and when it becomes due.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeferredWork {
    pub paper_id: String,
    pub kind: String,
    pub status: String,
    pub next_attempt_at: String,
    /// Whole seconds until then, as of plan time.
    pub retry_in_seconds: i64,
}

/// What one work's fetch established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchOutcome {
    /// Bytes arrived and were recorded: an `artefacts` row, and the want row
    /// retracted in the same transaction (`record_download`).
    Acquired,
    /// The walk ended without bytes. Whatever it concluded is in
    /// [`WorkOutcome::status`], and it is the *walk's* conclusion.
    Failed,
    /// The work could not be looked at all (no `papers` row). Nothing was
    /// established and nothing was written.
    NotAttempted,
}

/// One work's outcome, as `acquire` reports it.
///
/// [`Self::status`] is read back from the row the **walk** wrote, not chosen
/// here, so `acquire` cannot report a verdict it did not reach (#260).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkOutcome {
    pub paper_id: String,
    pub outcome: FetchOutcome,
    /// The `acquisition_state.status` the walk established, as stored. `None`
    /// once the gap is gone — a fetch that succeeded retracts the want.
    pub status: Option<String>,
    /// That row's `reason`: which routes were considered and what each said.
    pub reason: Option<String>,
    /// The gap's retry time after this run. `Some` only where `acquire`
    /// scheduled one: an `error`, which ADR-007 §2 routes to "retried with
    /// backoff by `acquire`".
    pub next_attempt_at: Option<String>,
    pub publisher: Option<String>,
    pub publisher_note: Option<String>,
    /// Which ladder step served the bytes (`artefacts.route`).
    pub route: Option<String>,
    /// What the walk could see in them.
    pub access: Option<String>,
    pub bytes: Option<usize>,
    pub path: Option<String>,
    /// The error `download_paper` returned, verbatim.
    pub error: Option<String>,
}

/// What `acquire` decided to do, and — unless it was a dry run — what
/// happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcquireReport {
    /// The decision, computed from the derived "have" and nothing else.
    pub plan: AcquirePlan,
    /// Empty for a dry run: no work was attempted, so there is no outcome.
    pub outcomes: Vec<WorkOutcome>,
}

impl AcquireReport {
    /// Works put on the wire.
    #[must_use]
    pub fn attempted(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.outcome != FetchOutcome::NotAttempted)
            .count()
    }

    /// Works that ended with recorded bytes.
    #[must_use]
    pub fn acquired(&self) -> usize {
        self.count(FetchOutcome::Acquired)
    }

    /// Works whose walk ended without bytes.
    #[must_use]
    pub fn failed(&self) -> usize {
        self.count(FetchOutcome::Failed)
    }

    fn count(&self, outcome: FetchOutcome) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.outcome == outcome)
            .count()
    }
}

/// The decision, before anything is fetched.
///
/// Every number here is derived on read. [`Self::held_total`] is the count of
/// wants ADR-007 §1 says we *already* have, and it is why a second run asks no
/// publisher anything: those works are not in [`Self::works`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcquirePlan {
    /// The resolved want kind. Always `"fulltext"`.
    pub kind: String,
    pub resume: bool,
    pub dry_run: bool,
    pub limit: Option<usize>,
    /// Recorded wants of `kind` the derivation says we do not hold.
    pub missing_total: usize,
    /// Recorded wants of `kind` already satisfied. **None of these will be
    /// fetched** — the acceptance criterion, stated as a number.
    pub held_total: usize,
    /// Of the missing ones, how many are held at a version the want rejects
    /// (ADR-007 §2's `wrong_version` shape).
    pub held_at_another_version: usize,
    /// Missing wants whose status has a human action, so they belong to
    /// `action_list` rather than to this queue.
    pub human_action_total: usize,
    /// Candidates after the queue-status filter, before `--resume` and
    /// `--limit`.
    pub queued_total: usize,
    /// The works this run will fetch, in `(kind, locator, paper_id)` order.
    pub works: Vec<PlannedWork>,
    /// Deferred by `--resume` because `next_attempt_at` is in the future.
    pub deferred: Vec<DeferredWork>,
    /// Candidates `--limit` dropped.
    pub truncated_by_limit: usize,
    /// `paper_ids` that matched no missing want: a work with no gap, or none
    /// of this kind. Named so an explicit request that did nothing says so.
    pub unmatched: Vec<String>,
}

/// `acquire_queue_add`: what enqueueing did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueAddReport {
    /// The ids asked for, in the order given.
    pub requested: Vec<String>,
    /// Enqueued: a `pending` want row this call created.
    pub enqueued: Vec<String>,
    /// Left exactly as it was — already queued, already waiting on a person,
    /// or already satisfied by the derivation.
    pub already_queued: Vec<String>,
    /// Named no such work.
    pub unknown: Vec<String>,
}

/// Everything that can go wrong before or around a run.
#[derive(Debug, thiserror::Error)]
pub enum AcquireError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Coverage(#[from] CoverageError),
    #[error(transparent)]
    Core(#[from] scitadel_core::error::CoreError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    /// A `kind` the ladder cannot close.
    #[error("{0}")]
    UnsupportedKind(String),
}

/// Refuse a kind the ladder cannot close, by name.
///
/// ADR-007 §3's routes are full-text routes, so an `si` / `table` / `figure`
/// want — or a want for one specific serialisation — is not something a fetch
/// can satisfy. Walking the ladder for one would record `error` on a want no
/// fetch could ever close: a status the walk did not establish in any useful
/// sense, and a row that can never resolve. Those gaps are closed by the file
/// arriving (`scan` / `attach`) or by a person, and the message says so.
fn check_kind(kind: &str) -> Result<(), AcquireError> {
    if kind == FULLTEXT_KIND {
        return Ok(());
    }
    Err(AcquireError::UnsupportedKind(format!(
        "{kind} is not a kind `acquire` can fetch: every ADR-007 §3 route is a full-text \
         route, so only `{FULLTEXT_KIND}` is acquirable. A {kind} gap is closed by the file \
         arriving — `scitadel scan` / `scitadel attach`, or a person — and asking for one here \
         would only record an `error` on a want no fetch can ever close."
    )))
}

/// Decide what to fetch, from the derived "have" and nothing else.
///
/// Pure reads: no write, no request. This is what `--dry-run` returns, and
/// what [`run`] walks.
pub fn plan(db: &Database, request: &AcquireRequest) -> Result<AcquirePlan, AcquireError> {
    let kind = request.resolved_kind();
    check_kind(kind)?;

    // One read, and the *only* derivation of "have" in this file. A want this
    // report does not list as missing is a want we hold, so it is not fetched.
    let report = db.coverage_report(None)?;
    let totals = report.by_kind.get(kind).copied().unwrap_or_default();
    // `filtered` is where the per-kind version-mismatch count comes from;
    // `by_kind`'s `held` count is not carried through a filter, which is why
    // both are taken from the same unfiltered report and the mismatch count
    // from a filtered view of it.
    let scoped = report
        .clone()
        .filtered(Some(kind))
        .map_err(CoverageError::from)?;

    let explicit = !request.paper_ids.is_empty();
    let requested: Vec<&str> = request.paper_ids.iter().map(String::as_str).collect();
    let mut human_action_total = 0usize;
    let mut queued_total = 0usize;
    let mut matched: HashSet<&str> = HashSet::new();
    let mut candidates: Vec<&MissingEntry> = Vec::new();

    for entry in &scoped.entries {
        if explicit {
            let Some(named) = requested
                .iter()
                .find(|id| entry.paper_id == **id || entry.paper_id.starts_with(**id))
            else {
                continue;
            };
            matched.insert(*named);
        } else if !is_queue_status(&entry.status) {
            // ADR-007 §2's action column: a row a person owns. Counted so a
            // dry run can say where they went instead of silently omitting
            // them — the same accounting `action_list` does.
            human_action_total += 1;
            continue;
        }
        queued_total += 1;
        candidates.push(entry);
    }

    let unmatched: Vec<String> = if explicit {
        requested
            .iter()
            .filter(|id| !matched.contains(*id))
            .map(|id| (*id).to_string())
            .collect()
    } else {
        Vec::new()
    };

    // `(kind, locator, paper_id)` is `coverage_report`'s own order and is
    // stable, so two runs over one library fetch in the same sequence — which
    // is what makes "the second run made no calls" a claim about a *specific*
    // work rather than about a set.
    let now = Utc::now();
    let mut works = Vec::new();
    let mut deferred = Vec::new();

    for entry in candidates {
        let doi = report.doi_of(&entry.paper_id).map(str::to_string);
        let (publisher, publisher_note) = publisher_of(doi.as_deref());
        let gap = db
            .acquisition_state(&entry.paper_id, kind, &entry.locator)
            .ok()
            .flatten();
        let next_attempt_at = gap.and_then(|g| g.next_attempt_at);
        if request.resume
            && next_attempt_at
                .as_deref()
                .is_some_and(|at| is_future(at, now))
        {
            let at = next_attempt_at.expect("checked just above");
            deferred.push(DeferredWork {
                paper_id: entry.paper_id.clone(),
                kind: entry.kind.clone(),
                status: entry.status.clone(),
                retry_in_seconds: seconds_until(&at, now),
                next_attempt_at: at,
            });
            continue;
        }
        works.push(PlannedWork {
            paper_id: entry.paper_id.clone(),
            kind: entry.kind.clone(),
            locator: entry.locator.clone(),
            wanted_version: entry.wanted_version.clone(),
            status: entry.status.clone(),
            next_attempt_at,
            publisher,
            publisher_note,
            doi,
        });
    }

    let truncated_by_limit = request
        .limit
        .map_or(0, |limit| works.len().saturating_sub(limit));
    if let Some(limit) = request.limit {
        works.truncate(limit);
    }

    Ok(AcquirePlan {
        kind: kind.to_string(),
        resume: request.resume,
        dry_run: request.dry_run,
        limit: request.limit,
        missing_total: totals.missing,
        held_total: totals.held,
        held_at_another_version: scoped.held_at_another_version,
        human_action_total,
        queued_total,
        works,
        deferred,
        truncated_by_limit,
        unmatched,
    })
}

/// Fetch every work [`plan`] selected.
///
/// A dry run returns the plan with an empty `outcomes` list and nothing else —
/// no request, no write.
///
/// A failed fetch is **not** propagated: a campaign that stops at the first
/// unreachable publisher is not a campaign. Each work's error is recorded on
/// its own [`WorkOutcome`] and the walk continues; only a failure to read or
/// write the library aborts the run, because then the report could not be
/// trusted about anything else in it.
pub async fn run(
    db: &Database,
    downloader: &PaperDownloader,
    papers_dir: &Path,
    request: &AcquireRequest,
) -> Result<AcquireReport, AcquireError> {
    let plan = plan(db, request)?;
    if request.dry_run {
        return Ok(AcquireReport {
            plan,
            outcomes: Vec::new(),
        });
    }

    let mut outcomes = Vec::with_capacity(plan.works.len());
    for work in &plan.works {
        outcomes.push(one(db, downloader, papers_dir, work).await?);
    }
    Ok(AcquireReport { plan, outcomes })
}

/// One work: read it, walk the ladder, report what the walk established.
async fn one(
    db: &Database,
    downloader: &PaperDownloader,
    papers_dir: &Path,
    work: &PlannedWork,
) -> Result<WorkOutcome, AcquireError> {
    let paper = match paper_of(db, &work.paper_id) {
        Ok(Some(paper)) => paper,
        Ok(None) => {
            return Ok(outcome(
                work,
                FetchOutcome::NotAttempted,
                None,
                Some(format!(
                    "no papers row for {}, so nothing was fetched and nothing was recorded",
                    work.paper_id
                )),
            ));
        }
        Err(e) => {
            return Ok(outcome(
                work,
                FetchOutcome::NotAttempted,
                None,
                Some(format!("could not read the work: {e}")),
            ));
        }
    };

    // ADR-007 §1 "Leases": claim the work before a single request leaves the
    // process, so two `acquire` runs over one library never fetch the same work
    // concurrently. A live holder is an ordinary outcome, reported as
    // `NotAttempted` rather than as a failure — see the ADR's "No row returned
    // means another live process holds the lease".
    let owner = scitadel_db::sqlite::new_lease_owner();
    if db
        .acquire_lease(&work.paper_id, &owner, LEASE_TTL_MS)?
        .is_none()
    {
        return Ok(outcome(
            work,
            FetchOutcome::NotAttempted,
            None,
            Some(format!(
                "another live process holds the lease on {}, so nothing was fetched",
                work.paper_id
            )),
        ));
    }

    tracing::info!(
        paper_id = %work.paper_id,
        status = %work.status,
        publisher = ?work.publisher,
        "acquire: walking the ladder"
    );

    // ADR-007 §1 "Leases": "The holder renews while fetching." The renewal runs
    // on a timer **while the request is in flight**, not after it — a paced
    // fetch against a publisher host is minutes long, and a claim that expired
    // in the middle of one is a claim a second process is entitled to take. The
    // fetch future is pinned once, so a renewal tick never restarts a request
    // that is already in progress.
    let fetch = downloader.download_paper(&paper, papers_dir);
    tokio::pin!(fetch);
    let mut renew = tokio::time::interval(std::time::Duration::from_millis(
        scitadel_db::sqlite::LEASE_RENEW_EVERY_MS,
    ));
    let fetched = loop {
        tokio::select! {
            result = &mut fetch => break result,
            _ = renew.tick() => {
                if !db.renew_lease(&work.paper_id, &owner, LEASE_TTL_MS)? {
                    // The work was taken over. Dropping the in-flight request is
                    // safe: nothing is written until `record_download` commits,
                    // which is after the whole body has arrived.
                    tracing::warn!(
                        paper_id = %work.paper_id,
                        "lost the lease mid-fetch; abandoning this attempt"
                    );
                    break Err(AdapterError::Other(format!(
                        "lost the lease on {} to another live process mid-fetch, so nothing was \
                         filed by this run",
                        work.paper_id
                    )));
                }
            }
        }
    };

    // The manifest, after the commit the ladder already made, and before the
    // claim is given up — ADR-007 §1: "written only by the lease holder, after
    // the DB commit". A mirror that cannot be written is not a failed fetch: the
    // database is right, and the next lease holder regenerates it.
    if fetched.is_ok()
        && let Err(e) = manifest::write(db, &paper, &owner)
    {
        tracing::warn!(error = %e, "could not write the manifest mirror");
    }
    let _ = db.release_lease(&work.paper_id, &owner);

    match fetched {
        Ok(result) => Ok(WorkOutcome {
            paper_id: work.paper_id.clone(),
            outcome: FetchOutcome::Acquired,
            status: None,
            reason: None,
            // The gap is gone: `record_download` retracted the want row in the
            // same transaction as the artefact, so there is nothing to retry.
            next_attempt_at: None,
            publisher: work.publisher.clone(),
            publisher_note: work.publisher_note.clone(),
            route: Some(result.route.label().to_string()),
            access: Some(result.access.to_string()),
            bytes: Some(result.bytes),
            path: Some(result.path.display().to_string()),
            error: None,
        }),
        Err(error) => {
            // The status is the walk's, read back off the row it wrote — never
            // one chosen here (#260).
            let gap = db.acquisition_state(&work.paper_id, &work.kind, &work.locator)?;
            let next_attempt_at = schedule_retry(db, gap.as_ref())?;
            let mut out = outcome(work, FetchOutcome::Failed, gap, Some(error.to_string()));
            out.next_attempt_at = next_attempt_at;
            Ok(out)
        }
    }
}

/// `acquire --from <queue.ndjson>`: import raid's queue, then drain it.
///
/// The import runs **first** and separately from the drain, because it is a
/// reconciliation of a file and the drain is a reconciliation of the library —
/// two different idempotence stories. Importing them would make a dry run
/// ambiguous: `--dry-run` promises no writes, and a queue import is a write.
pub async fn run_from_ndjson(
    db: &Database,
    downloader: &PaperDownloader,
    papers_dir: &Path,
    request: &AcquireRequest,
    source: &Path,
) -> Result<(NdjsonImport, AcquireReport), AcquireError> {
    if request.dry_run {
        return Err(AcquireError::UnsupportedKind(format!(
            "`--from {}` imports a queue, which writes `acquisition_state` rows, so it cannot              run under --dry-run: the flag promises no writes. Run it without --dry-run to import              the queue, then `scitadel acquire --dry-run` to see what would be fetched.",
            source.display()
        )));
    }
    let import = import_ndjson_queue(db, source)?;
    let report = run(db, downloader, papers_dir, request).await?;
    Ok((import, report))
}

/// A [`WorkOutcome`] with everything but the retry time filled in.
fn outcome(
    work: &PlannedWork,
    fetch: FetchOutcome,
    gap: Option<StateRow>,
    error: Option<String>,
) -> WorkOutcome {
    WorkOutcome {
        paper_id: work.paper_id.clone(),
        outcome: fetch,
        status: gap.as_ref().map(|g| g.status.clone()),
        reason: gap.as_ref().and_then(|g| g.reason.clone()),
        next_attempt_at: None,
        publisher: work.publisher.clone(),
        publisher_note: work.publisher_note.clone(),
        route: None,
        access: None,
        bytes: None,
        path: None,
        error,
    }
}

/// Write `next_attempt_at` for a gap whose established status is `error`, and
/// return the value written.
///
/// The time is anchored to `updated_at` — *when the walk established the
/// failure* — rather than to the clock now, and that is what makes a re-run
/// over an unchanged failure write nothing at all: the ladder's own re-write
/// is skipped by `write_acquisition_states`' `WHERE` guard, so `updated_at`
/// stands still and the same anchor yields the same stamp. A failure that
/// established something new moves `updated_at`, and the schedule moves with
/// it, which is the correct behaviour.
///
/// `rate_limited` is left alone: its `next_attempt_at` came from the bucket
/// ledger, and a work-level backoff must not overwrite a publisher's own
/// answer. Every other status is not retried at all — ADR-007 §2 gives them
/// no action, or an action for a person.
///
/// Every other column is written back **as it stands** (via
/// [`StateRow::to_write`]), and `publisher` in particular is not re-asserted
/// from this module's classification: the row was not created here, the ladder
/// writes `None` on purpose because it evaluated no route, and re-asserting it
/// would make every re-run a write (the ladder clears it again each time).
fn schedule_retry(db: &Database, gap: Option<&StateRow>) -> Result<Option<String>, DbError> {
    let Some(gap) = gap else {
        return Ok(None);
    };
    if gap.status != "error" {
        return Ok(None);
    }
    let next = parse(&gap.updated_at)
        .map_or_else(Utc::now, |established| {
            established + chrono::Duration::minutes(RETRY_BACKOFF_MINUTES)
        })
        .to_rfc3339();
    if gap.next_attempt_at.as_deref() == Some(next.as_str()) {
        // Already exactly this stamp — the `WHERE` guard would skip the write
        // anyway; saying so is what keeps "a re-run writes nothing" visible.
        return Ok(Some(next));
    }
    let mut row = gap.to_write();
    row.next_attempt_at = Some(next.clone());
    let mut conn = db.conn()?;
    write_acquisition_states(&mut conn, &[row])?;
    Ok(Some(next))
}

/// Enqueue works for `acquire`: record a `pending` full-text want for each.
///
/// ADR-007 §2 calls `pending` "the `acquire` queue", so enqueueing *is*
/// writing that status — the one status whose whole meaning is "never tried,
/// picked up again by `acquire`". This is the one place `acquire` writes a
/// status, and it does so for a want nobody has stated yet.
///
/// **An existing want row is left exactly as it is.** A `needs_ill` row is a
/// human decision; returning it to `pending` because someone re-queued the
/// work would erase the reason the work is waiting on a person. The report
/// says `already_queued` so the caller can see which half happened.
///
/// **A work whose full text we already hold is still enqueued**, and this is
/// not a bug in the check that was left out: the ADR-007 §1 derivation
/// immediately reads the new want as satisfied, so it lands in `coverage`'s
/// `held` count, never in its `missing` one, and [`plan`] — which walks the
/// missing wants — never fetches it. Asking "do we already have it?" here
/// would mean a *second* derivation of "have" for a case the first one
/// already handles, and #260/#261/#275 are what that costs. A redundant
/// enqueue therefore costs one row and zero requests; a second derivation
/// costs a bug nobody tests for.
pub fn queue_add(db: &Database, paper_ids: &[String]) -> Result<QueueAddReport, AcquireError> {
    let mut out = QueueAddReport {
        requested: paper_ids.to_vec(),
        enqueued: Vec::new(),
        already_queued: Vec::new(),
        unknown: Vec::new(),
    };
    for id in paper_ids {
        // Per-item rather than a hard error: this is a batch enqueue, and one
        // bad id must not leave the other works unqueued with no record of
        // which half happened. The report names it under `unknown`.
        let Some(paper) = paper_of(db, id)? else {
            out.unknown.push(id.clone());
            continue;
        };
        let paper_id = paper.id.as_str().to_string();
        if db
            .acquisition_state(&paper_id, FULLTEXT_KIND, FULLTEXT_LOCATOR)?
            .is_some()
        {
            out.already_queued.push(paper_id);
            continue;
        }
        let doi = paper.doi.as_deref();
        let (publisher, _) = publisher_of(doi);
        let mut conn = db.conn()?;
        write_acquisition_states(
            &mut conn,
            &[StateWrite {
                paper_id: paper_id.clone(),
                kind: FULLTEXT_KIND.to_string(),
                locator: FULLTEXT_LOCATOR.to_string(),
                wanted_version: WANTED_VERSION.to_string(),
                status: STATUS_PENDING.to_string(),
                reason: Some(
                    "queued for acquisition by `acquire_queue_add`; no full text is recorded yet"
                        .to_string(),
                ),
                // Named only when the DOI's registrant prefix is classified.
                // `None` is the answer we are sure of otherwise (#261), and it
                // is why `action_list` still derives the group from the DOI
                // rather than reading this column.
                publisher,
                hint_url: None,
                // No file for a human to drop: this row is closed by a fetch,
                // which is what separates it from the flat importer's.
                drop_path: None,
                // Due now.
                next_attempt_at: None,
                updated_at: Utc::now().to_rfc3339(),
            }],
        )?;
        out.enqueued.push(paper_id);
    }
    Ok(out)
}

/// One work's `papers` row, by exact id or unambiguous prefix.
///
/// The prefix arm exists because every other scitadel surface accepts one
/// (#232): an id a tool printed must work on the way back in.
pub(crate) fn paper_of(
    db: &Database,
    id: &str,
) -> Result<Option<Paper>, scitadel_core::error::CoreError> {
    let (paper_repo, _, _, _, _) = db.repositories();
    if let Some(paper) = paper_repo.get(id)? {
        return Ok(Some(paper));
    }
    let matches: Vec<Paper> = paper_repo
        .list_all(10_000, 0)?
        .into_iter()
        .filter(|p| p.id.as_str().starts_with(id))
        .collect();
    match matches.len() {
        0 => Ok(None),
        // Naming one of two works would acquire the wrong paper, so this is
        // the caller's error to fix rather than a coin flip.
        1 => Ok(matches.into_iter().next()),
        _ => Err(scitadel_core::error::CoreError::AmbiguousPrefix {
            entity: "paper".to_string(),
            prefix: id.to_string(),
            count: matches.len(),
        }),
    }
}

/// Does ADR-007 §2 route this status to `acquire` for a fetch?
fn is_queue_status(status: &str) -> bool {
    ACQUIRE_QUEUE_STATUSES.contains(&status)
        // A status this build does not know is never guessed into the queue:
        // `coverage.rs` reports such a row separately precisely so it cannot be
        // lost, and fetching for a status we cannot describe is acting on one.
        && status_vocab(status).is_some()
}

/// Is `stamp` in the future relative to `now`?
///
/// An unparseable stamp is **not** treated as future. A corrupt timestamp must
/// not make a work permanently unfetchable, and the fetch that eventually runs
/// corrects the row.
fn is_future(stamp: &str, now: DateTime<Utc>) -> bool {
    parse(stamp).is_some_and(|at| at > now)
}

/// Whole seconds from `now` until `stamp`; `0` when it is unparseable.
fn seconds_until(stamp: &str, now: DateTime<Utc>) -> i64 {
    parse(stamp).map_or(0, |at| (at - now).num_seconds().max(0))
}

/// Parse an RFC 3339 timestamp, `None` when it is not one.
fn parse(stamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(stamp)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// The publisher a gap row may name for `doi`, and the note for why there is
/// none.
///
/// The ladder's own implementation of the two halves of `PublisherVerdict`
/// (see [`crate::download::publisher_columns`]), filtered so a **malformed**
/// DOI produces no note either: there is no registrant prefix to be unknown
/// *about*, and a note naming prefix `<not-a-doi>` would print a prefix that
/// does not exist — the exact thing #261's audit had to unpick.
fn publisher_of(doi: Option<&str>) -> (Option<String>, Option<String>) {
    crate::download::publisher_columns(doi.filter(|d| validate_doi(d)))
}
#[cfg(test)]
mod tests {
    //! The acceptance tests for `acquire`, driven through the real ladder
    //! against a real on-disk library. Only the pacer (a counting one) and the
    //! network (one `wiremock` server every endpoint points at) are
    //! substituted — the same substitution `download.rs`'s own tests use, so a
    //! change to either crate cannot pass these by accident.
    //!
    //! The one that matters is
    //! `a_second_run_makes_no_network_calls_for_held_artefacts`: it asserts a
    //! **request count**, not a row count. "The rows look the same afterwards"
    //! is satisfied by a re-run that re-downloaded everything and wrote the
    //! same rows back, which is the failure this command exists to prevent.

    use super::*;

    use async_trait::async_trait;
    use scitadel_core::config::OpenAlexAuth;
    use scitadel_core::ports::{Bucket, Cost, PaceDenied, PaceTier, Pacer, Permit};
    use scitadel_db::sqlite::{ACCESS_BASIS_MANUAL, ArtefactWrite, BlobWrite, WriteMode};
    use scitadel_http::{BucketPolicyTable, PacedClient};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PDF_BYTES: &[u8] = b"%PDF-1.7\nacquired body\n%%EOF\n";
    /// Unclassified registrant prefix — deliberately not in the registry.
    const UNKNOWN_DOI: &str = "10.99999/some.suffix.12345";
    /// Classified (Nature), for the contrast half of the publisher test.
    const NATURE_DOI: &str = "10.1038/s41586-020-2649-2";

    /// Grants everything, immediately. The ledger's behaviour is
    /// `scitadel-db`'s to test, not this file's.
    #[derive(Debug, Default)]
    struct GrantingPacer;

    #[async_trait]
    impl Pacer for GrantingPacer {
        async fn acquire(
            &self,
            bucket: &Bucket,
            tier: PaceTier,
            _cost: Cost,
        ) -> Result<Permit, PaceDenied> {
            Ok(Permit {
                bucket: bucket.clone(),
                tier,
                not_before: std::time::Instant::now(),
            })
        }
    }

    /// A real library on disk with the whole ladder pointed at one server.
    ///
    /// The `TempDir` is held for the fixture's lifetime because the pool
    /// opens connections lazily: dropping it mid-test would let a later
    /// `conn()` create a fresh, empty database.
    struct Fixture {
        dir: tempfile::TempDir,
        db: Database,
        server: MockServer,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let db = Database::open(&dir.path().join("scitadel.db")).expect("open db");
            db.migrate().expect("migrate");
            Self {
                dir,
                db,
                server: MockServer::start().await,
            }
        }

        /// Where the legacy `papers/<stem>.<ext>` copy is written.
        fn papers_dir(&self) -> std::path::PathBuf {
            self.dir.path().join("papers")
        }

        fn downloader(&self) -> PaperDownloader {
            let client = PacedClient::new(
                PacedClient::default_transport().expect("transport builds"),
                std::sync::Arc::new(GrantingPacer),
                BucketPolicyTable::new(),
            );
            let base = self.server.uri();
            PaperDownloader::with_client(
                self.db.clone(),
                OpenAlexAuth {
                    email: "polite@example.org".to_string(),
                    api_key: "openalex-secret-key".to_string(),
                },
                client,
                crate::download::Endpoints {
                    unpaywall_api: format!("{base}/v2"),
                    doi_resolver: format!("{base}/doi"),
                    arxiv_pdf: format!("{base}/pdf"),
                    biorxiv_content: format!("{base}/content"),
                    openalex_api: format!("{base}/works"),
                    osti_base: base.clone(),
                },
            )
        }

        /// Serve an OA PDF reachable through Unpaywall for `doi` — the
        /// smallest walk that actually succeeds.
        async fn serve_unpaywall_pdf(&self, doi: &str) {
            let pdf_route = format!("/oa/{}.pdf", doi.replace('/', "_"));
            let pdf_url = format!("{}{pdf_route}", self.server.uri());
            self.serve(
                &format!("/v2/{doi}"),
                format!(r#"{{"best_oa_location":{{"url_for_pdf":"{pdf_url}"}}}}"#),
            )
            .await;
            self.serve(&pdf_route, String::from_utf8_lossy(PDF_BYTES).to_string())
                .await;
        }

        async fn serve(&self, route: &str, body: String) {
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .mount(&self.server)
                .await;
        }

        /// A 404 for a path **including** its query string — wiremock refuses to
        /// match a `?` in a path, so this splits it into a path plus a
        /// `query_param` matcher.
        async fn miss_with_query(&self, route: &str) {
            let (route_path, query) = route.split_once('?').expect("a query string");
            let (key, value) = query.split_once('=').expect("k=v");
            Mock::given(method("GET"))
                .and(path(route_path))
                .and(query_param(key, value))
                .respond_with(ResponseTemplate::new(404))
                .mount(&self.server)
                .await;
        }

        async fn miss(&self, route: &str) {
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(404))
                .mount(&self.server)
                .await;
        }

        /// Every request anything in the process put on the wire.
        async fn request_count(&self) -> usize {
            self.server
                .received_requests()
                .await
                .expect("wiremock recorded")
                .len()
        }

        /// Serve an OpenAlex `/works/{id}` answer with the three facts the
        /// pre-fetch identity check reads — `title`, `publication_year` and the
        /// first authorship — plus an OA location the leg can fetch.
        ///
        /// `title` and `author` take [`None`] to leave the field out entirely,
        /// which is the shape that produces `unverified` rather than `ok`.
        async fn serve_openalex_work(
            &self,
            openalex_id: &str,
            title: Option<&str>,
            year: Option<i32>,
            author: Option<&str>,
        ) {
            let pdf_route = format!("/oa/{openalex_id}.pdf");
            let pdf_url = format!("{}{pdf_route}", self.server.uri());
            let field = |name: &str, value: Option<String>| match value {
                Some(value) => {
                    let json = serde_json::to_string(&value).expect("json");
                    format!(r#""{name}":{json},"#)
                }
                None => String::new(),
            };
            let authorships = match author {
                Some(author) => {
                    let json = serde_json::to_string(&author).expect("json");
                    format!(r#""authorships":[{{"author":{{"display_name":{json}}}}}],"#)
                }
                None => String::new(),
            };
            self.serve(
                &format!("/works/{openalex_id}"),
                format!(
                    "{{{}{}{}\"best_oa_location\":{{\"pdf_url\":{}}}}}",
                    field("title", title.map(str::to_string)),
                    field("publication_year", year.map(|year| year.to_string()),),
                    authorships,
                    serde_json::to_string(&pdf_url).expect("json")
                ),
            )
            .await;
            self.serve(&pdf_route, String::from_utf8_lossy(PDF_BYTES).to_string())
                .await;
        }

        /// A saved `Paper` whose title, year and first author the identity check
        /// reads, plus whatever identifiers the walk needs.
        fn paper_with(
            &self,
            id: &str,
            title: &str,
            year: Option<i32>,
            authors: &[&str],
            doi: Option<&str>,
            openalex_id: Option<&str>,
        ) {
            let mut p = Paper::new(title);
            p.id = scitadel_core::models::PaperId::from(id.to_string());
            p.year = year;
            p.authors = authors.iter().map(|author| (*author).to_string()).collect();
            p.doi = doi.map(str::to_string);
            p.openalex_id = openalex_id.map(str::to_string);
            let (paper_repo, _, _, _, _) = self.db.repositories();
            paper_repo.save(&p).expect("save paper");
        }

        /// `paper_identity_checks` as `(phase, status, expected, resolved)` per row.
        fn identity_checks(&self) -> Vec<(String, String, Option<String>, Option<String>)> {
            let conn = self.db.conn().expect("conn");
            let mut stmt = conn
                .prepare(
                    "SELECT phase, status, expected_title, resolved_title
                       FROM paper_identity_checks ORDER BY paper_id, phase",
                )
                .expect("prepare");
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .expect("query")
                .map(Result::unwrap)
                .collect()
        }

        /// How many `blobs` rows exist — the other half of "nothing was filed",
        /// since an artefact's bytes are its blob's.
        fn count_blobs(&self) -> i64 {
            let conn = self.db.conn().expect("conn");
            conn.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                .expect("count")
        }

        /// `acquisition_attempts` as `(route, outcome, detail)`.
        fn attempts(&self) -> Vec<(String, String, Option<String>)> {
            let conn = self.db.conn().expect("conn");
            let mut stmt = conn
                .prepare("SELECT route, outcome, detail FROM acquisition_attempts ORDER BY id")
                .expect("prepare");
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .expect("query")
                .map(Result::unwrap)
                .collect()
        }

        /// A saved `Paper` with the identifiers a test needs.
        fn paper(&self, id: &str, doi: Option<&str>) -> Paper {
            let mut p = Paper::new(format!("Paper {id}"));
            p.id = scitadel_core::models::PaperId::from(id.to_string());
            p.doi = doi.map(str::to_string);
            let (paper_repo, _, _, _, _) = self.db.repositories();
            paper_repo.save(&p).expect("save paper");
            p
        }

        /// Queue a gap row the way `acquire_queue_add` writes one.
        fn gap(
            &self,
            paper_id: &str,
            status: &str,
            drop_path: Option<&str>,
            next_attempt_at: Option<&str>,
        ) {
            let mut conn = self.db.conn().expect("conn");
            write_acquisition_states(
                &mut conn,
                &[StateWrite {
                    paper_id: paper_id.into(),
                    kind: FULLTEXT_KIND.into(),
                    locator: FULLTEXT_LOCATOR.into(),
                    wanted_version: WANTED_VERSION.into(),
                    status: status.into(),
                    reason: Some(format!("seeded {status}")),
                    publisher: None,
                    hint_url: None,
                    drop_path: drop_path.map(str::to_string),
                    next_attempt_at: next_attempt_at.map(str::to_string),
                    updated_at: "2026-01-01T00:00:00+00:00".into(),
                }],
            )
            .expect("write gap");
        }

        /// Hold a VoR full text for `paper_id`, bytes and all.
        fn hold_fulltext(&self, paper_id: &str) {
            let sha = format!("sha-{paper_id}");
            self.db
                .write_artefacts(
                    &[ArtefactWrite {
                        id: String::new(),
                        paper_id: paper_id.into(),
                        kind: "fulltext_pdf".into(),
                        version: "vor".into(),
                        locator: String::new(),
                        sha256: Some(sha.clone()),
                        format: Some("pdf".into()),
                        access_status: "full_text".into(),
                        route: "publisher".into(),
                        access_basis: ACCESS_BASIS_MANUAL.into(),
                        label: None,
                        caption: None,
                        source_url: None,
                        publisher: None,
                        publisher_note: None,
                        imported_from: None,
                        retrieved_at: "2026-01-01T00:00:00+00:00".into(),
                        missing_on_disk: false,
                        blob: Some(BlobWrite {
                            sha256: sha,
                            bytes: 4,
                            mime: "application/pdf".into(),
                            rel_path: format!("blobs/{paper_id}.pdf"),
                            created_at: "2026-01-01T00:00:00+00:00".into(),
                        }),
                    }],
                    WriteMode::Reconcile,
                )
                .expect("hold fulltext");
        }

        /// The whole `acquisition_state` table, row for row, column for
        /// column — the shape an idempotence claim is made against.
        fn state_rows(&self) -> Vec<String> {
            let conn = self.db.conn().expect("conn");
            let mut stmt = conn
                .prepare(
                    "SELECT paper_id, kind, locator, wanted_version, status, reason, publisher,
                            hint_url, drop_path, next_attempt_at, updated_at
                     FROM acquisition_state ORDER BY paper_id, kind, locator",
                )
                .expect("prepare");
            let rows = stmt
                .query_map([], |r| {
                    Ok(format!(
                        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, String>(10)?,
                    ))
                })
                .expect("query");
            rows.map(Result::unwrap).collect()
        }

        /// `(kind, version, access_status, route)` per artefact row.
        fn artefacts(&self) -> Vec<String> {
            let conn = self.db.conn().expect("conn");
            let mut stmt = conn
                .prepare(
                    "SELECT paper_id, kind, version, access_status, route, sha256 IS NOT NULL,
                            missing_on_disk
                     FROM artefacts ORDER BY paper_id, kind, locator",
                )
                .expect("prepare");
            let rows = stmt
                .query_map([], |r| {
                    Ok(format!(
                        "{}|{}|{}|{}|{}|{}|{}",
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                })
                .expect("query");
            rows.map(Result::unwrap).collect()
        }

        /// Every `papers` row, whole, as one comparable string.
        ///
        /// This replaces a `legacy_columns` helper that read only
        /// `local_path` / `download_status` / `last_attempt_at`. Those three are no
        /// longer written by anything (#253's S2e), so asserting they had not moved
        /// tested nothing: it would have passed against an implementation that never
        /// touched them in the first place, which is exactly the property the dry-run
        /// test exists to check. Whole-row equality subsumes the three and cannot
        /// rot that way.
        fn papers_rows(&self) -> Vec<String> {
            let conn = self.db.conn().expect("conn");
            let mut stmt = conn
                .prepare("SELECT * FROM papers ORDER BY id")
                .expect("prepare");
            let names: Vec<String> = stmt
                .column_names()
                .iter()
                .map(|n| (*n).to_string())
                .collect();
            let rows = stmt
                .query_map([], |r| {
                    let values = names
                        .iter()
                        .map(|name| {
                            format!("{name}={:?}", r.get::<_, Option<String>>(name.as_str()))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Ok(format!("{{ {values} }}"))
                })
                .expect("query");
            rows.map(Result::unwrap).collect()
        }

        async fn acquire(&self, request: &AcquireRequest) -> AcquireReport {
            run(&self.db, &self.downloader(), &self.papers_dir(), request)
                .await
                .expect("acquire runs")
        }
    }

    /// An RFC 3339 stamp `minutes` from now.
    fn at(minutes: i64) -> String {
        (Utc::now() + chrono::Duration::minutes(minutes)).to_rfc3339()
    }

    // =====================================================================
    // 1. The acceptance criterion.
    // =====================================================================

    /// #253's headline: **a re-run makes no network calls for artefacts
    /// already held.**
    ///
    /// Asserted on `received_requests().len()`, so an implementation that
    /// re-downloaded everything and happened to write the same rows back would
    /// fail this. Two works close two different ways, and both have to be
    /// skipped by the same derivation:
    ///
    /// - `p-fetched` is closed by `record_download`, which **deletes** the
    ///   want row in the same transaction as the artefact;
    /// - `p-imported` is closed by holding the file while its want row
    ///   survives — it carries a `drop_path`, so the retraction statement
    ///   (scoped to `drop_path IS NULL`) is not its to fire. A work like this
    ///   is only skipped if the "have" derivation is consulted, and it is
    ///   mounted on a URL that would happily serve it, so a second request
    ///   would be counted rather than merely possible.
    #[tokio::test]
    async fn a_second_run_makes_no_network_calls_for_held_artefacts() {
        let fx = Fixture::new().await;
        fx.serve_unpaywall_pdf(UNKNOWN_DOI).await;
        fx.serve_unpaywall_pdf(NATURE_DOI).await;
        fx.paper("p-fetched", Some(UNKNOWN_DOI));
        fx.paper("p-imported", Some(NATURE_DOI));
        fx.gap("p-fetched", "pending", None, None);
        // The flat importer's shape: a gap with a `drop_path`, closed by a
        // file arriving rather than by a fetch.
        fx.gap(
            "p-imported",
            "pending",
            Some("/lib/papers/stem/fulltext.pdf"),
            None,
        );
        fx.hold_fulltext("p-imported");

        // The pre-existing file is already satisfied, so the first run must
        // not fetch it either — that is the whole claim, and starting the
        // count here makes the second run's zero meaningful.
        let first = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(
            first.plan.held_total, 1,
            "the held want is counted as held, not as missing"
        );
        assert_eq!(
            first
                .plan
                .works
                .iter()
                .map(|w| w.paper_id.as_str())
                .collect::<Vec<_>>(),
            vec!["p-fetched"],
            "only the work we do not hold is planned"
        );
        assert_eq!(first.acquired(), 1);
        let after_first = fx.request_count().await;
        assert!(after_first > 0, "the first run did fetch something");

        let second = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(
            second.plan.works.len(),
            0,
            "nothing is left to fetch: both wants are satisfied"
        );
        assert_eq!(second.outcomes.len(), 0);
        assert_eq!(
            fx.request_count().await,
            after_first,
            "the second run put nothing on the wire"
        );
    }

    // =====================================================================
    // 1. #253: no artefact is ever filed under a mismatched identity.
    // =====================================================================

    /// The criterion, asserted on the table itself: **a mismatch writes no
    /// `artefacts` row.**
    ///
    /// Byte-identical before and after — every column of every row, not a count —
    /// because "the rows look the same" is satisfied by a run that filed the
    /// wrong document and then deleted it, and by a run that filed the right one
    /// under a different id.
    ///
    /// The mismatch is a **served page** naming another paper: the shape ADR-007
    /// §3's post-fetch check exists for ("this catches redirects to the wrong
    /// paper"), and the one an index cannot be blamed for.
    #[tokio::test]
    async fn a_mismatch_writes_no_artefact_row() {
        let fx = Fixture::new().await;
        fx.paper_with(
            "p-mismatch",
            "Deep learning for radiopharmaceutical image reconstruction",
            Some(2020),
            &["Young, Christopher J."],
            Some(UNKNOWN_DOI),
            None,
        );
        fx.gap("p-mismatch", "pending", None, None);
        // Unpaywall names no location; the DOI resolver lands on a page whose
        // own metadata says it is a different article.
        fx.miss_with_query(&format!("/v2/{UNKNOWN_DOI}?email=polite@example.org"))
            .await;
        fx.serve(
            &format!("/doi/{UNKNOWN_DOI}"),
            r#"<!doctype html><html><head>
                 <meta name="citation_title" content="Total-body PET scanners for theranostics">
                 <meta name="citation_publication_date" content="2019-04-01">
                 <meta name="citation_author" content="Harris, James M.">
                 </head><body>an entirely different article</body></html>"#
                .to_string(),
        )
        .await;

        let before = fx.artefacts();
        assert!(before.is_empty(), "nothing is held to begin with");

        let report = fx.acquire(&AcquireRequest::queue()).await;

        assert_eq!(report.acquired(), 0, "nothing was filed");
        assert_eq!(
            fx.artefacts(),
            before,
            "#253: a mismatch must leave the artefacts table exactly as it found it"
        );
        assert_eq!(
            fx.count_blobs(),
            0,
            "and no blob either — the bytes were discarded, not stored"
        );

        // The other half of "a mismatch": the reason has to be *visible*.
        let checks = fx.identity_checks();
        assert_eq!(checks.len(), 1, "the check is recorded: {checks:?}");
        let (phase, status, expected, resolved) = &checks[0];
        assert_eq!(phase, "post_fetch");
        assert_eq!(status, "mismatch");
        assert_eq!(
            expected.as_deref(),
            Some("Deep learning for radiopharmaceutical image reconstruction"),
            "both titles are stored, or the row cannot be acted on"
        );
        assert_eq!(
            resolved.as_deref(),
            Some("Total-body PET scanners for theranostics")
        );

        // ADR-007 §2's status for exactly this, which is a human action — so
        // `action_list` prints it and `acquire` never re-fetches it.
        let gap = fx
            .db
            .acquisition_state("p-mismatch", FULLTEXT_KIND, FULLTEXT_LOCATOR)
            .expect("read gap")
            .expect("a gap row exists");
        assert_eq!(gap.status, "identity_mismatch");
        assert!(
            gap.reason
                .as_deref()
                .is_some_and(|reason| reason.contains("theranostics")),
            "the reason names the served title: {:?}",
            gap.reason
        );

        // And the audit row: a fetch happened and deliberately filed nothing.
        let attempts = fx.attempts();
        assert_eq!(attempts.len(), 1, "{attempts:?}");
        assert_eq!(attempts[0].1, "identity_mismatch");
        assert!(
            attempts[0]
                .2
                .as_deref()
                .is_some_and(|detail| detail.contains("theranostics")),
            "{attempts:?}"
        );
    }

    /// The schema's whole point is being able to see the disagreement, so this is
    /// the pre-fetch half of it: the **stored** paper's title against the one
    /// OpenAlex resolved for it.
    ///
    /// Both titles, verbatim, on the row — which is what ADR-007 §2's
    /// `identity_mismatch` action ("check DOI (both titles shown)") is a promise
    /// about.
    #[tokio::test]
    async fn a_pre_fetch_mismatch_is_recorded_with_both_titles() {
        let fx = Fixture::new().await;
        fx.paper_with(
            "p-openalex",
            "Deep learning for radiopharmaceutical image reconstruction",
            Some(2020),
            &["Young, Christopher J."],
            None,
            Some("W123"),
        );
        fx.gap("p-openalex", "pending", None, None);
        // OpenAlex answers with a different article's title for this id.
        fx.serve_openalex_work(
            "W123",
            Some("Total-body PET scanners for theranostics"),
            Some(2021),
            Some("Harris, James M."),
        )
        .await;

        let report = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(report.acquired(), 0);
        assert!(fx.artefacts().is_empty(), "nothing filed");

        let checks = fx.identity_checks();
        assert_eq!(checks.len(), 1, "{checks:?}");
        let (phase, status, expected, resolved) = &checks[0];
        assert_eq!(
            phase, "pre_fetch",
            "the mismatch is caught before the bytes"
        );
        assert_eq!(status, "mismatch");
        assert_eq!(
            expected.as_deref(),
            Some("Deep learning for radiopharmaceutical image reconstruction"),
            "the expected title is the stored paper's"
        );
        assert_eq!(
            resolved.as_deref(),
            Some("Total-body PET scanners for theranostics"),
            "and the resolved one is the registry's"
        );
    }

    /// The opposite of the criterion: a check that could not be *confirmed* must
    /// not block acquisition, or the ladder would stop working for every work
    /// whose PDF carries no readable `/Title` — and `unverified` is a property of
    /// the data, so a blocked fetch would return it again forever.
    ///
    /// The shape here is the real one: OpenAlex names the work correctly but
    /// records no year and no author, so the titles match and **nothing
    /// corroborates** them. The verdict is `unverified`, the row says so, and the
    /// PDF is filed.
    ///
    /// This is deliberately the opposite rule to [`a_mismatch_writes_no_artefact_row`],
    /// and the difference is the whole design: `mismatch` is positive evidence
    /// that the bytes are another work, `unverified` is no evidence at all.
    #[tokio::test]
    async fn an_unverified_check_still_lets_the_fetch_proceed() {
        let fx = Fixture::new().await;
        fx.paper_with(
            "p-unverified",
            "Deep learning for radiopharmaceutical image reconstruction",
            Some(2020),
            &[],
            None,
            Some("W999"),
        );
        fx.gap("p-unverified", "pending", None, None);
        fx.serve_openalex_work(
            "W999",
            Some("Deep learning for radiopharmaceutical image reconstruction"),
            None,
            None,
        )
        .await;

        let report = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(report.acquired(), 1, "an unverified check is not a block");
        assert_eq!(
            fx.artefacts().len(),
            1,
            "the full text is filed: the check did not establish a mismatch"
        );

        let checks = fx.identity_checks();
        assert_eq!(checks.len(), 1, "{checks:?}");
        assert_eq!(checks[0].0, "pre_fetch");
        assert_eq!(
            checks[0].1, "unverified",
            "and it is recorded as `unverified`, not as a pass: {:?}",
            checks[0]
        );
        assert!(
            fx.attempts().is_empty(),
            "nothing was refused, so nothing is recorded as refused"
        );
    }

    /// #253's escape hatch, end to end: a person settles the identity, and a
    /// **later run** fetches and files the work instead of re-asking.
    ///
    /// Asserted in both directions, because "write-once" is only a claim about
    /// machines if a person's own later `--reason` still replaces their earlier
    /// one, and only a claim about machines if a machine's second mismatch is not
    /// recorded over the override.
    #[tokio::test]
    async fn an_override_survives_a_later_run() {
        let fx = Fixture::new().await;
        fx.paper_with(
            "p-override",
            "Deep learning for radiopharmaceutical image reconstruction",
            Some(2020),
            &["Young, Christopher J."],
            Some(UNKNOWN_DOI),
            None,
        );
        fx.gap("p-override", "pending", None, None);
        fx.miss_with_query(&format!("/v2/{UNKNOWN_DOI}?email=polite@example.org"))
            .await;
        fx.serve(
            &format!("/doi/{UNKNOWN_DOI}"),
            r#"<meta name="citation_title" content="Total-body PET scanners for theranostics">"#
                .to_string(),
        )
        .await;

        // Run 1: refused, and the work is parked on a person.
        assert_eq!(fx.acquire(&AcquireRequest::queue()).await.acquired(), 0);
        assert!(
            fx.artefacts().is_empty(),
            "run 1 filed nothing: the gate said no"
        );

        // The person checks it by hand. A blank reason is refused outright.
        assert!(
            matches!(
                fx.db.override_identity("p-override", "   "),
                Err(scitadel_db::sqlite::IdentityError::ReasonRequired)
            ),
            "an override without a reason is not an override"
        );
        let report = fx
            .db
            .override_identity("p-override", "same report; OpenAlex has the older title")
            .expect("override");
        assert_eq!(
            report.gap_status_before.as_deref(),
            Some("identity_mismatch"),
            "the work was blocked, and the override says so"
        );
        assert_eq!(report.gap_status_after.as_deref(), Some("pending"));

        // Run 2: the same bytes, the same mismatch — and it is filed this time,
        // because a person said the identity question was already settled.
        assert_eq!(
            fx.acquire(&AcquireRequest::queue()).await.acquired(),
            1,
            "a settled identity must not block the work for ever"
        );
        assert_eq!(fx.artefacts().len(), 1);
        assert!(
            fx.identity_checks()
                .iter()
                .all(|(_, status, _, _)| status == "overridden"),
            "and the machine's second verdict was not recorded over the override: {:?}",
            fx.identity_checks()
        );

        // A person may change their own mind; only machines are locked out.
        fx.db
            .override_identity("p-override", "checked again against the OSTI record")
            .expect("re-override");
        let checks = fx.identity_checks();
        assert_eq!(
            checks.len(),
            2,
            "one row per phase, still — a second `--reason` updates rather than inserts: {checks:?}"
        );
        assert!(
            checks
                .iter()
                .all(|(_, status, _, _)| status == "overridden"),
            "{checks:?}"
        );
    }

    /// ADR-007 §3 step 3: a DOE report with no DOI is fetched from OSTI's
    /// deterministic `purl` URL, and a body that is not a PDF is never filed.
    ///
    /// Both halves, because the second is the one that would be invisible
    /// otherwise: OSTI answers an id it does not have with **404 and a 265 kB
    /// HTML page of its own site** (probed 2026-10-04), so a route that trusted
    /// the status code would hand `coverage` a "full text" nobody can read.
    #[tokio::test]
    async fn the_osti_route_fetches_a_pdf_and_fails_closed_on_an_html_error_page() {
        let fx = Fixture::new().await;
        let title = "IDC Re-Engineering Phase 2 Glossary";
        // A report with no DOI at all — the only identifier is `osti_id`, which is
        // exactly the class the route exists for.
        fx.paper_with("p-osti", title, Some(2016), &[], None, None);
        fx.db.set_osti_id("p-osti", "1234567").expect("set osti_id");
        fx.gap("p-osti", "pending", None, None);
        fx.serve(
            "/servlets/purl/1234567",
            format!("%PDF-1.6\n/Title({title})\nbody\n%%EOF\n"),
        )
        .await;

        let report = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(report.acquired(), 1, "the report is fetched and filed");
        let artefacts = fx.artefacts();
        assert_eq!(artefacts.len(), 1, "{artefacts:?}");
        assert!(
            artefacts[0].contains("osti"),
            "the artefact records the route that served it: {artefacts:?}"
        );

        // The failing half: an HTML page under the same route.
        let fx = Fixture::new().await;
        fx.paper_with("p-osti-html", title, Some(2016), &[], None, None);
        fx.db
            .set_osti_id("p-osti-html", "999999999")
            .expect("set osti_id");
        fx.gap("p-osti-html", "pending", None, None);
        fx.serve(
            "/servlets/purl/999999999",
            "<!DOCTYPE html><html lang=\"en\"><title>Page not found</title>\
             <body>265 kB of site chrome</body></html>"
                .to_string(),
        )
        .await;

        let report = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(report.acquired(), 0, "an HTML page is not a report");
        assert!(
            fx.artefacts().is_empty(),
            "and it is not filed as one: {:?}",
            fx.artefacts()
        );
        assert_eq!(fx.count_blobs(), 0, "no blob for a body that is not a PDF");
        // The walk continued past it (there was nothing else to try), and said so
        // rather than calling the report unavailable — the route was asked and
        // could not be resolved.
        let gap = fx
            .db
            .acquisition_state("p-osti-html", FULLTEXT_KIND, FULLTEXT_LOCATOR)
            .expect("read gap")
            .expect("a gap row exists");
        assert_eq!(
            gap.status, "error",
            "OSTI could not be resolved, which is `error` and not `unavailable`: {:?}",
            gap.reason
        );
        assert!(
            gap.reason
                .as_deref()
                .is_some_and(|reason| reason.contains("not a PDF")),
            "the reason says why: {:?}",
            gap.reason
        );
    }

    // =====================================================================
    // 2. `--dry-run`.
    // =====================================================================

    /// A dry run writes nothing and calls nothing.
    ///
    /// "Writes nothing" is checked as whole-table equality — every column of
    /// `acquisition_state`, every artefact row, and every column of every `papers`
    /// row — because a count would miss an in-place update, which is the shape an
    /// idempotence bug takes. Whole `papers` rows rather than the three retired
    /// download columns: nothing writes those any more, so checking them would
    /// pass vacuously.
    #[tokio::test]
    async fn dry_run_writes_nothing_and_calls_nothing() {
        let fx = Fixture::new().await;
        fx.serve_unpaywall_pdf(UNKNOWN_DOI).await;
        fx.paper("p-queued", Some(UNKNOWN_DOI));
        fx.paper("p-human", Some(NATURE_DOI));
        fx.gap("p-queued", "pending", None, None);
        fx.gap("p-human", "needs_ill", None, None);

        let states = fx.state_rows();
        let artefacts = fx.artefacts();
        let papers = fx.papers_rows();

        let report = fx.acquire(&AcquireRequest::queue().planning()).await;

        // It says what it *would* do: one work, and the human-action row
        // accounted for rather than dropped.
        assert_eq!(report.plan.works.len(), 1);
        assert_eq!(report.plan.works[0].paper_id, "p-queued");
        assert_eq!(report.plan.human_action_total, 1);
        assert_eq!(report.plan.queued_total, 1);
        assert!(report.plan.dry_run);
        assert_eq!(
            report.outcomes.len(),
            0,
            "a dry run attempts nothing, so it has no outcomes to report"
        );

        assert_eq!(
            fx.request_count().await,
            0,
            "a dry run put nothing on the wire"
        );
        assert_eq!(fx.state_rows(), states, "acquisition_state is unchanged");
        assert_eq!(fx.artefacts(), artefacts, "artefacts is unchanged");
        assert_eq!(fx.papers_rows(), papers, "no `papers` row moved, at all");
    }

    // =====================================================================
    // 3. `--resume`, and the column that makes it possible.
    // =====================================================================

    /// `--resume` defers a `rate_limited` work until its `next_attempt_at`.
    ///
    /// This is the test that proves Part 1 was needed: with no way to write
    /// `next_attempt_at`, a `rate_limited` row could only ever be *read*, and
    /// the deferral it describes would be unrepresentable — `--resume` would
    /// re-fetch the work on every run and the ADR-007 §2 meaning of the status
    /// would be decorative.
    ///
    /// The mount is live, so "no request" is a measurement rather than an
    /// absence of capability.
    #[tokio::test]
    async fn resume_defers_a_rate_limited_work_until_next_attempt_at() {
        let fx = Fixture::new().await;
        fx.serve_unpaywall_pdf(UNKNOWN_DOI).await;
        fx.paper("p-limited", Some(UNKNOWN_DOI));
        fx.gap("p-limited", "rate_limited", None, Some(&at(60)));

        let deferred = fx.acquire(&AcquireRequest::queue().resuming()).await;
        assert_eq!(
            deferred.plan.works.len(),
            0,
            "a retry time an hour out is not due"
        );
        assert_eq!(deferred.plan.deferred.len(), 1);
        let work = &deferred.plan.deferred[0];
        assert_eq!(work.paper_id, "p-limited");
        assert_eq!(work.status, "rate_limited");
        assert!(
            work.retry_in_seconds > 3_000,
            "the deferral says when it is due: {}s",
            work.retry_in_seconds
        );
        assert_eq!(
            fx.request_count().await,
            0,
            "a deferred work must not be put on the wire at all"
        );

        // The same work, due: it runs. The stamp is rewritten through the same
        // column, which is the writer half of the round trip.
        let mut conn = fx.db.conn().expect("conn");
        write_acquisition_states(
            &mut conn,
            &[StateWrite {
                paper_id: "p-limited".into(),
                kind: FULLTEXT_KIND.into(),
                locator: FULLTEXT_LOCATOR.into(),
                wanted_version: WANTED_VERSION.into(),
                status: "rate_limited".into(),
                reason: Some("the bucket was in backoff; it has since expired".into()),
                publisher: None,
                hint_url: None,
                drop_path: None,
                next_attempt_at: Some(at(-60)),
                updated_at: "2026-01-01T00:00:00+00:00".into(),
            }],
        )
        .expect("rewrite the retry time");

        let due = fx.acquire(&AcquireRequest::queue().resuming()).await;
        assert_eq!(due.plan.deferred.len(), 0, "a past retry time is due now");
        assert_eq!(due.plan.works.len(), 1);
        assert_eq!(due.acquired(), 1);
        assert!(
            fx.request_count().await > 0,
            "a due work is fetched; nothing was deferred this time"
        );
    }

    // =====================================================================
    // 4. Idempotence.
    // =====================================================================

    /// A re-run writes the same rows, with the same `updated_at`.
    ///
    /// Three runs, because the claim is worth more than "the second run did
    /// nothing" — the first is the acquisition, the second **re-fetches**
    /// and must still write nothing, and the third is `--resume`, which
    /// defers the retry the first run scheduled and therefore does not fetch
    /// at all.
    ///
    /// Two independent guards have to hold for the re-fetching run:
    ///
    /// - the ladder re-records its gap with a fresh `updated_at`, and
    ///   `write_acquisition_states`' `WHERE` guard skips the write because
    ///   nothing else differs — the walk's `reason` is byte-identical because
    ///   the mocks are;
    /// - `acquire`'s retry stamp is anchored to that `updated_at`
    ///   ([`schedule_retry`]), so the second run computes the *same*
    ///   `next_attempt_at` and its own write is skipped too. A clock-based
    ///   stamp would move on every run, and this test is what says it must not.
    #[tokio::test]
    async fn acquire_is_idempotent() {
        let fx = Fixture::new().await;
        fx.serve_unpaywall_pdf(UNKNOWN_DOI).await;
        // Every route for this one misses, so the walk records a gap rather
        // than bytes: the interesting half is the row that must not move.
        fx.miss("/content/10.1101/2024.11.19.624167v1.full.pdf")
            .await;
        fx.miss("/content/10.1101/2024.11.19.624167.full.pdf").await;
        fx.serve("/v2/10.1101/2024.11.19.624167", "{}".to_string())
            .await;
        fx.miss("/doi/10.1101/2024.11.19.624167").await;

        fx.paper("p-got", Some(UNKNOWN_DOI));
        fx.paper("p-missed", Some("10.1101/2024.11.19.624167"));
        fx.gap("p-got", "pending", None, None);
        fx.gap("p-missed", "pending", None, None);

        let first = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(first.acquired(), 1, "one work was acquired");
        assert_eq!(first.failed(), 1, "one work recorded a gap");
        let states = fx.state_rows();
        let artefacts = fx.artefacts();

        // The first run scheduled a retry, anchored to when it established the
        // failure rather than to the clock — which is what lets the re-fetch
        // below write nothing.
        let gap = fx
            .db
            .acquisition_state("p-missed", FULLTEXT_KIND, FULLTEXT_LOCATOR)
            .expect("read the gap")
            .expect("a gap was recorded");
        assert_eq!(gap.status, "error");
        assert_eq!(
            gap.next_attempt_at.as_deref(),
            Some(
                (parse(&gap.updated_at).expect("the ladder stamped a parseable time")
                    + chrono::Duration::minutes(RETRY_BACKOFF_MINUTES))
                .to_rfc3339()
                .as_str()
            ),
            "the retry is anchored to when the failure was established, not to \
             the clock at write time — otherwise a re-run would move it"
        );

        // An explicit run retries a due `error` — that is ADR-007 §2's "retried
        // with backoff", and "due" here means the caller did not pass
        // `--resume`. It must still write nothing.
        let second = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(
            second
                .plan
                .works
                .iter()
                .map(|w| w.paper_id.as_str())
                .collect::<Vec<_>>(),
            vec!["p-missed"],
            "the acquired work is gone from the queue; the failed one is due"
        );
        assert!(second.failed() == 1, "the retry failed the same way");
        assert_eq!(fx.state_rows(), states, "a re-fetch changed no gap row");
        assert_eq!(fx.artefacts(), artefacts, "a re-fetch changed no artefact");

        // `--resume` reads the stamp the first run wrote: not due, so not
        // fetched, and not written either.
        let resumed = fx.acquire(&AcquireRequest::queue().resuming()).await;
        assert_eq!(resumed.plan.deferred.len(), 1);
        assert_eq!(resumed.plan.deferred[0].paper_id, "p-missed");
        assert_eq!(
            resumed.outcomes.len(),
            0,
            "a deferred work is not attempted at all"
        );
        assert_eq!(fx.state_rows(), states, "--resume changed no gap row");
    }

    // =====================================================================
    // 5. Publisher honesty.
    // =====================================================================

    /// An unclassified publisher records no publisher and no route verdict.
    ///
    /// Both halves of #261, at the only place `acquire` fills the column:
    ///
    /// - the stored `acquisition_state.publisher` is `NULL`, and the note
    ///   beside it is [`RouteVerdict::PublisherUnknown`]'s wording verbatim;
    /// - no TDM verdict appears anywhere — the report for the unclassified work
    ///   and the report for a classified one differ only in the publisher name,
    ///   so "no TDM route available" cannot be printed for a publisher nobody
    ///   looked at.
    ///
    /// The classified work is in the same test on purpose: without it, `None`
    /// would also be what a *broken* writer produces for every DOI, and the
    /// assertion would pass for the wrong reason.
    #[tokio::test]
    async fn an_unclassified_publisher_records_no_publisher_and_no_route_verdict() {
        let fx = Fixture::new().await;
        fx.paper("p-unknown", Some(UNKNOWN_DOI));
        fx.paper("p-known", Some(NATURE_DOI));

        let queued = queue_add(&fx.db, &["p-unknown".into(), "p-known".into()]).expect("enqueue");
        assert_eq!(queued.enqueued, vec!["p-unknown", "p-known"]);

        let unknown = fx
            .db
            .acquisition_state("p-unknown", FULLTEXT_KIND, FULLTEXT_LOCATOR)
            .expect("read")
            .expect("queued");
        assert_eq!(
            unknown.publisher, None,
            "an unclassified prefix must never get a guessed publisher"
        );
        assert_eq!(unknown.next_attempt_at, None, "queued works are due now");

        let plan = plan(&fx.db, &AcquireRequest::queue()).expect("plan");
        let unknown_work = plan
            .works
            .iter()
            .find(|w| w.paper_id == "p-unknown")
            .expect("planned");
        assert_eq!(unknown_work.publisher, None);
        let note = unknown_work
            .publisher_note
            .as_deref()
            .expect("an explanation is carried");
        assert_eq!(
            note,
            scitadel_core::publisher::RouteVerdict::PublisherUnknown {
                prefix: "99999".to_string()
            }
            .note(),
            "the note is that type's wording, verbatim"
        );
        assert!(
            !note.contains("no TDM route available"),
            "nothing evaluated a TDM route, so nothing may report one: {note}"
        );
        for name in ["Nature", "Elsevier", "Springer", "Wiley", "Unknown"] {
            assert!(!note.contains(name), "{note} names a publisher: {name}");
        }

        // The contrast: the same walk classifies the registry's answer and
        // stores it, so `None` above is about classification and not about the
        // column being unreachable.
        let known = fx
            .db
            .acquisition_state("p-known", FULLTEXT_KIND, FULLTEXT_LOCATOR)
            .expect("read")
            .expect("queued");
        assert_eq!(
            known.publisher.as_deref(),
            Some("nature"),
            "a classified prefix is stored by its stable label"
        );
        let known_work = plan
            .works
            .iter()
            .find(|w| w.paper_id == "p-known")
            .expect("planned");
        assert_eq!(known_work.publisher.as_deref(), Some("nature"));
        assert_eq!(
            known_work.publisher_note, None,
            "a classified publisher needs no explanation"
        );
    }

    // =====================================================================
    // 6. The #260 bug, through the queue runner.
    // =====================================================================

    /// An index naming no location never yields `unavailable`.
    ///
    /// `download.rs` already pins this for the ladder. It is pinned again here
    /// because `acquire` is the component that *reports* the verdict to a
    /// person or an agent, and a queue runner that summarised a
    /// no-location walk as "this work has no full text" would be the third
    /// writer of a claim nobody established.
    ///
    /// The walk: the preprint transform 404s, OpenAlex answers 200 with
    /// `best_oa_location: null`, Unpaywall answers 200 with no
    /// `url_for_pdf`, and doi.org 404s. Two routes named nothing — a fact
    /// about two indexes and about nothing else.
    #[tokio::test]
    async fn an_index_naming_no_location_never_yields_unavailable() {
        let fx = Fixture::new().await;
        fx.miss("/content/10.1101/2024.11.19.624167v1.full.pdf")
            .await;
        fx.miss("/content/10.1101/2024.11.19.624167.full.pdf").await;
        fx.serve(
            "/works/W123",
            r#"{"id":"W123","best_oa_location":null,"open_access":{"is_oa":false}}"#.to_string(),
        )
        .await;
        fx.serve("/v2/10.1101/2024.11.19.624167", "{}".to_string())
            .await;
        fx.miss("/doi/10.1101/2024.11.19.624167").await;

        fx.paper("p-index", Some("10.1101/2024.11.19.624167"));
        fx.paper("p-openalex", None);
        // The work has both identifiers, so OpenAlex is consulted as well.
        let conn = fx.db.conn().expect("conn");
        conn.execute(
            "UPDATE papers SET openalex_id = 'W123' WHERE id = 'p-openalex'",
            [],
        )
        .expect("set openalex id");
        drop(conn);
        fx.gap("p-index", "pending", None, None);
        fx.gap("p-openalex", "pending", None, None);

        let report = fx.acquire(&AcquireRequest::queue()).await;
        assert_eq!(report.failed(), 2, "neither walk obtained bytes");

        for id in ["p-index", "p-openalex"] {
            let gap = fx
                .db
                .acquisition_state(id, FULLTEXT_KIND, FULLTEXT_LOCATOR)
                .expect("read")
                .expect("the walk recorded a gap");
            assert_ne!(
                gap.status, "unavailable",
                "{id}: two indices that named nothing are not evidence about access"
            );
            assert_eq!(
                gap.status, "error",
                "{id}: routes were tried and could not be resolved — retryable"
            );
            let reason = gap.reason.expect("a reason is recorded");
            assert!(
                reason.contains("not evidence that the work is unavailable"),
                "{id}: the index's silence must say what it is: {reason}"
            );
            let outcome = report
                .outcomes
                .iter()
                .find(|o| o.paper_id == id)
                .expect("an outcome");
            assert_eq!(outcome.status.as_deref(), Some("error"));
            assert_ne!(outcome.status.as_deref(), Some("unavailable"));
        }

        // And nothing anywhere in the library claims it.
        assert!(
            !fx.state_rows().iter().any(|r| r.contains("|unavailable|")),
            "no row carries `unavailable`: {:?}",
            fx.state_rows()
        );
    }

    // =====================================================================
    // The queue's shape.
    // =====================================================================

    /// `--kind` refuses a kind the ladder cannot close, by name.
    ///
    /// Fetching for an `si` want would record an `error` on a gap no fetch can
    /// ever close, which is a status nobody established and a row that can
    /// never resolve.
    #[test]
    fn a_kind_the_ladder_cannot_close_is_refused_by_name() {
        for kind in ["si", "table", "figure", "fulltext_pdf", "fulltext_html"] {
            let err = plan(
                &Database::open_in_memory().expect("db"),
                &AcquireRequest::queue().of_kind(kind),
            )
            .expect_err("must refuse");
            let message = err.to_string();
            assert!(
                message.contains(kind),
                "the message names the kind: {message}"
            );
            assert!(
                message.contains("scan") && message.contains("attach"),
                "the message says how such a gap is actually closed: {message}"
            );
        }
        // And the one kind that is acquirable is not refused.
        assert!(check_kind(FULLTEXT_KIND).is_ok());
    }

    /// `--limit` truncates after the deferral, so it never spends the budget on
    /// a work that is not due.
    #[tokio::test]
    async fn limit_applies_to_due_works_and_says_what_it_dropped() {
        let fx = Fixture::new().await;
        for (id, doi) in [
            ("p-a", "10.1101/aaa"),
            ("p-b", "10.1101/bbb"),
            ("p-c", "10.1101/ccc"),
        ] {
            fx.paper(id, Some(doi));
            fx.gap(id, "pending", None, Some(&at(-10)));
        }
        let report = fx
            .acquire(&AcquireRequest::queue().resuming().limited(2).planning())
            .await;
        assert_eq!(report.plan.queued_total, 3);
        assert_eq!(report.plan.deferred.len(), 0, "all three are due");
        assert_eq!(report.plan.works.len(), 2);
        assert_eq!(report.plan.truncated_by_limit, 1);
        // `coverage_report`'s stable order, so which two is deterministic.
        assert_eq!(
            report
                .plan
                .works
                .iter()
                .map(|w| w.paper_id.as_str())
                .collect::<Vec<_>>(),
            vec!["p-a", "p-b"]
        );
    }

    /// An explicit id that matches no missing want is reported, not silently
    /// ignored — and one that matches does not also drain the queue.
    #[tokio::test]
    async fn an_explicit_id_narrows_the_run_and_names_itself_when_it_matches_nothing() {
        let fx = Fixture::new().await;
        fx.serve_unpaywall_pdf(UNKNOWN_DOI).await;
        fx.paper("p-queued", Some(UNKNOWN_DOI));
        fx.paper("p-other", Some(NATURE_DOI));
        fx.gap("p-queued", "pending", None, None);
        fx.gap("p-other", "pending", None, None);

        let named = plan(
            &fx.db,
            &AcquireRequest::for_papers(["p-queued".to_string()]),
        )
        .expect("plan");
        assert_eq!(
            named
                .works
                .iter()
                .map(|w| w.paper_id.as_str())
                .collect::<Vec<_>>(),
            vec!["p-queued"],
            "naming a work narrows the run to it"
        );
        assert!(named.unmatched.is_empty());

        // A prefix resolves, because every other scitadel surface accepts one.
        let prefixed =
            plan(&fx.db, &AcquireRequest::for_papers(["p-oth".to_string()])).expect("plan");
        assert_eq!(prefixed.works[0].paper_id, "p-other");

        // A work nobody wants a full text for is named as unmatched rather
        // than acquiring it: inventing a want for an untracked work is what
        // `coverage`'s untracked bucket exists to avoid.
        fx.paper("p-untracked", Some("10.1101/untracked"));
        let unmatched = plan(
            &fx.db,
            &AcquireRequest::for_papers(["p-untracked".to_string()]),
        )
        .expect("plan");
        assert!(unmatched.works.is_empty());
        assert_eq!(unmatched.unmatched, vec!["p-untracked".to_string()]);

        // A work whose status belongs to a person is left for `action_list`
        // when no ids are named, and fetched when one is.
        fx.gap("p-other", "needs_ill", None, None);
        let human = plan(&fx.db, &AcquireRequest::queue()).expect("plan");
        assert_eq!(
            human
                .works
                .iter()
                .map(|w| w.paper_id.as_str())
                .collect::<Vec<_>>(),
            vec!["p-queued"],
            "the needs_ill row is not in the fetch queue"
        );
        assert_eq!(human.human_action_total, 1);
        let overridden =
            plan(&fx.db, &AcquireRequest::for_papers(["p-other".to_string()])).expect("plan");
        assert_eq!(
            overridden.works.len(),
            1,
            "naming the work is the caller overriding the queue filter"
        );
    }

    /// Enqueueing is idempotent and never overwrites a row a person owns.
    #[tokio::test]
    async fn queue_add_is_idempotent_and_leaves_a_humans_row_alone() {
        let fx = Fixture::new().await;
        fx.paper("p-plain", Some(UNKNOWN_DOI));
        fx.paper("p-held", Some(NATURE_DOI));
        fx.paper("p-human", Some("10.1016/j.jneumeth.2005.09.009"));
        fx.gap("p-human", "needs_ill", None, None);
        fx.hold_fulltext("p-held");

        let first = queue_add(
            &fx.db,
            &[
                "p-plain".to_string(),
                "p-held".to_string(),
                "p-human".to_string(),
                "p-nope".to_string(),
            ],
        )
        .expect("enqueue");
        assert_eq!(
            first.enqueued,
            vec!["p-plain", "p-held"],
            "a work with no want row is queued, whether or not we hold it yet"
        );
        assert_eq!(
            first.already_queued,
            vec!["p-human"],
            "a row a person owns is left exactly as it is"
        );
        assert_eq!(first.unknown, vec!["p-nope"]);
        let rows = fx.state_rows();
        assert_eq!(rows.len(), 3, "one row per work that named one: {rows:?}");
        assert!(
            rows.iter().any(|r| r.contains("\"needs_ill\"")),
            "the human's row kept its status: {rows:?}"
        );

        // Enqueueing a work we already hold costs a row and nothing else: the
        // derivation reads the new want as satisfied, so it is never missing
        // and `acquire` never fetches it. That is the same "have" the report
        // uses, which is why this needed no second derivation.
        let report = fx.db.coverage_report(None).expect("coverage");
        assert!(
            report.entries.iter().all(|e| e.paper_id != "p-held"),
            "a want we already satisfy must never be reported missing: {:?}",
            report.entries
        );
        assert!(
            report
                .by_kind
                .get(FULLTEXT_KIND)
                .is_some_and(|k| k.wanted == 3 && k.held == 1 && k.missing == 2),
            "three wants: one satisfied, one queued, one waiting on a person: {:?}",
            report.by_kind
        );
        let planned = plan(&fx.db, &AcquireRequest::queue()).expect("plan");
        assert_eq!(
            planned
                .works
                .iter()
                .map(|w| w.paper_id.as_str())
                .collect::<Vec<_>>(),
            vec!["p-plain"],
            "the satisfied want is not fetched, and the human's row is not ours"
        );
    }

    /// #253's last acceptance criterion: "raid's `p1`/`p3` NDJSON acquisition
    /// fields import via `acquire --from works.ndjson` (curation statuses stay
    /// raid's)".
    ///
    /// Asserted on both halves, because the flag is one sentence with a clause in
    /// each half:
    ///
    /// - **imports** — the gap arrives and becomes *actionable*, which is the part
    ///   that could silently not happen. [`plan`] reads the same table this
    ///   importer writes, so the imported work shows up in `plan.works` with
    ///   nothing else changed. That is the criterion's "so an imported gap becomes
    ///   actionable by the existing plan".
    /// - **curation statuses stay raid's** — a raid status scitadel has no word
    ///   for is stored verbatim in the gap's `reason` and listed in the report,
    ///   while `status` holds `pending`. A reader who assumed the importer
    ///   re-labelled raid's verdicts would find the verdict text intact and the
    ///   report naming it; one who assumed scitadel dropped it would find it in
    ///   `reason`.
    #[tokio::test]
    async fn acquire_from_ndjson_imports_the_gap_and_preserves_raids_status() {
        let fx = Fixture::new().await;
        // A work scitadel already holds, with the DOI raid's queue file names.
        fx.paper("p-raid", Some(UNKNOWN_DOI));

        let queue = fx.dir.path().join("works.ndjson");
        std::fs::write(
            &queue,
            // A blank line, one line naming a work this library does not have, and
            // one raid row: a top-level curation status raid invented, one
            // per-artefact status that happens to be one of ADR-007 §2's own
            // words, and a field this build does not read.
            // NDJSON: one complete object per line. The raid row is deliberately
            // one long line, because a pretty-printed object is two lines and the
            // format does not allow it.
            format!(
                "\n{{\"doi\": \"10.99999/not-in-this-library\", \"artefacts\": [\"fulltext\"]}}\n\
                 {{\"doi\": \"https://doi.org/{UNKNOWN_DOI}\", \"curation_status\": \
                 \"accepted_by_reviewer\", \"curation_note\": \"screen\", \"artefacts\": \
                 [{{\"kind\": \"fulltext\"}}, {{\"kind\": \"si\", \"label\": \"Supporting \
                 Information S1\", \"status\": \"needs_ill\", \"hint_url\": \
                 \"https://example.org/s1\"}}]}}\n"
            ),
        )
        .expect("write the queue");

        // Nothing is queued yet: the import is what creates the gaps.
        assert!(
            plan(&fx.db, &AcquireRequest::queue())
                .expect("plan")
                .works
                .is_empty()
        );

        let import = import_ndjson_queue(&fx.db, &queue).expect("import");

        // ---- the import half ----
        assert_eq!(import.rows_read, 2, "{import:?}");
        assert_eq!(import.gaps_written, 2, "one fulltext want, one si want");
        assert_eq!(
            import.works,
            vec!["p-raid".to_string()],
            "and both belong to the one work raid named"
        );
        assert_eq!(
            import.unknown,
            vec!["10.99999/not-in-this-library".to_string()],
            "a work this library does not hold is reported, not created"
        );

        // ---- "curation statuses stay raid's" ----
        // Every curation status the import read is listed, in file order.
        assert_eq!(import.preserved_statuses.len(), 2, "{import:?}");

        // The fulltext want inherited the row-level status, which is raid's own
        // vocabulary and has no scitadel equivalent.
        let fulltext = &import.preserved_statuses[0];
        assert_eq!(fulltext.kind, "fulltext");
        assert_eq!(
            fulltext.raid_status, "accepted_by_reviewer",
            "character for character — raid's own word, never normalised"
        );
        assert_eq!(
            fulltext.stored_as, "pending",
            "`pending` is a claim about scitadel — never tried by us — not a translation of \
             raid's verdict"
        );
        assert!(
            fulltext.in_reason,
            "and raid's own words are preserved verbatim beside it"
        );

        // The SI's status was already ADR-007 §2's own word, so it passes through
        // with nothing else to preserve.
        let si = &import.preserved_statuses[1];
        assert_eq!(si.kind, "si");
        assert_eq!(si.locator, "supporting-information-s1");
        assert_eq!(si.raid_status, "needs_ill");
        assert_eq!(
            si.stored_as, "needs_ill",
            "raid's word, verbatim — not re-spelled, not re-cased"
        );
        assert!(
            !si.in_reason,
            "and `status` already carries it, so `reason` adds nothing"
        );

        // Read the rows back off disk: the verdict must be *in the database*, not
        // only in the report.
        let fulltext_gap = fx
            .db
            .acquisition_state("p-raid", FULLTEXT_KIND, FULLTEXT_LOCATOR)
            .expect("read")
            .expect("a want");
        assert_eq!(fulltext_gap.status, "pending");
        let reason = fulltext_gap
            .reason
            .clone()
            .expect("the verdict was preserved");
        assert!(
            reason.contains("accepted_by_reviewer"),
            "raid's curation status survives verbatim: {reason}"
        );
        assert!(
            !fulltext_gap.status.contains("review"),
            "and nothing invented a scitadel-sounding status for it"
        );

        let si_gap = fx
            .db
            .acquisition_state("p-raid", "si", "supporting-information-s1")
            .expect("read")
            .expect("a want");
        assert_eq!(
            si_gap.status, "needs_ill",
            "raid's word, stored as it stands"
        );
        assert_eq!(
            si_gap.hint_url.as_deref(),
            Some("https://example.org/s1"),
            "and the acquisition half imported too"
        );

        // ---- the field this build does not read is reported, not dropped ----
        assert_eq!(
            import.unknown_fields.iter().cloned().collect::<Vec<_>>(),
            vec!["curation_note".to_string()],
            "#247's \"covers raid's queue files without loss\" is a question about this list"
        );

        // ---- "actionable by the existing plan" ----
        // The point of the criterion: the imported want is in the queue that
        // `acquire` already drains, with no new code path between them. `plan` is
        // pure reads, so the request count stays at zero and this says nothing
        // about the network.
        let actionable = plan(&fx.db, &AcquireRequest::queue()).expect("plan");
        assert_eq!(
            actionable
                .works
                .iter()
                .map(|w| w.paper_id.as_str())
                .collect::<Vec<_>>(),
            vec!["p-raid"],
            "the imported fulltext want is queued for the ladder, and nothing else is"
        );
        assert_eq!(actionable.works[0].status, "pending");
        assert_eq!(
            fx.request_count().await,
            0,
            "planning is pure reads, so this measured nothing about the wire"
        );

        // The `si` want imported too, and lands where ADR-007 §2 says it does: not
        // in the fetch queue (no fetch closes an `si`), but in `action_list`.
        let action_list = fx.db.action_list().expect("action_list");
        assert!(
            action_list
                .groups
                .iter()
                .any(|group| group.count() == 1 && group.status == "needs_ill"),
            "the imported SI want is a human action: {:?}",
            action_list.groups
        );

        // And re-importing the same file writes nothing.
        let before = fx.state_rows();
        let again = import_ndjson_queue(&fx.db, &queue).expect("re-import");
        assert_eq!(again.gaps_written, 0);
        assert_eq!(
            again.untouched_existing_gaps, 2,
            "both wants were left as they stood"
        );
        assert_eq!(fx.state_rows(), before, "an unchanged queue writes nothing");
    }

    /// `--from` under `--dry-run` is refused rather than quietly writing: the flag
    /// promises no writes, and an import is a write.
    /// ADR-007 §1 "Leases" as the fetch path uses it: a work another live
    /// process holds is **not fetched at all**, and a work this run fetches is
    /// claimed and then released.
    ///
    /// Both halves, because "no row returned means another live process holds the
    /// lease" is only useful if the second half is not quietly skipped — a runner
    /// that ignored the claim would be the two-process failure the table exists to
    /// prevent, and it would still pass a test that only checked the refusal.
    ///
    /// The mount is live for the refused work, so "not fetched" is a measurement.
    #[tokio::test]
    async fn a_work_another_process_holds_is_not_fetched_and_our_claim_is_released() {
        let fx = Fixture::new().await;
        fx.serve_unpaywall_pdf(UNKNOWN_DOI).await;
        fx.serve_unpaywall_pdf(NATURE_DOI).await;
        fx.paper("p-free", Some(UNKNOWN_DOI));
        fx.paper("p-taken", Some(NATURE_DOI));
        fx.gap("p-free", "pending", None, None);
        fx.gap("p-taken", "pending", None, None);

        // Another process holds one of them, with a live lease.
        let holder = scitadel_db::sqlite::new_lease_owner();
        assert!(
            fx.db
                .acquire_lease("p-taken", &holder, LEASE_TTL_MS)
                .unwrap()
                .is_some()
        );

        let report = fx.acquire(&AcquireRequest::queue()).await;
        // Both works are planned — `plan` reads the gaps, not the leases — but
        // only one is attempted. A runner that dropped the leased work from the
        // plan instead would report it as "nothing to do", which reads like a
        // healthy queue; reporting it as `NotAttempted` with the reason is what
        // makes "another process has it" visible.
        let leased = report
            .outcomes
            .iter()
            .find(|o| o.paper_id == "p-taken")
            .expect("an outcome for the leased work");
        assert_eq!(leased.outcome, FetchOutcome::NotAttempted);
        assert_eq!(leased.status, None, "and nothing was written for it");
        assert!(
            leased
                .error
                .as_deref()
                .is_some_and(|e| e.contains("holds the lease")),
            "and it says why: {leased:?}"
        );
        assert_eq!(report.acquired(), 1, "the free work was acquired");
        assert_eq!(
            report.attempted(),
            1,
            "`attempted` counts the work put on the wire, and the leased work never was"
        );

        // Every lease this run took is gone again, so a later run is not blocked
        // by this one having happened.
        let held = fx.db.leases().unwrap();
        assert_eq!(
            held.iter().map(|l| l.paper_id.as_str()).collect::<Vec<_>>(),
            vec!["p-taken"],
            "only the other process's claim survives: {held:?}"
        );

        // And the manifest for the work we fetched is on disk, written after the
        // commit by the claim this run held.
        let (paper_repo, _, _, _, _) = fx.db.repositories();
        let paper = paper_repo.get("p-free").unwrap().unwrap();
        let mirror = crate::manifest::manifest_path(fx.dir.path(), &paper);
        assert!(mirror.exists(), "{}", mirror.display());
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mirror).unwrap()).unwrap();
        assert_eq!(body["work"]["paper_id"], "p-free");
        assert_eq!(
            body["artefacts"].as_array().map(Vec::len),
            Some(1),
            "and it describes the artefact the commit left behind"
        );
    }

    #[tokio::test]
    async fn a_queue_import_is_refused_under_dry_run() {
        let fx = Fixture::new().await;
        let queue = fx.dir.path().join("works.ndjson");
        std::fs::write(&queue, "{}\n").unwrap();
        let err = run_from_ndjson(
            &fx.db,
            &fx.downloader(),
            &fx.papers_dir(),
            &AcquireRequest::queue().planning(),
            &queue,
        )
        .await
        .expect_err("refused");
        assert!(
            err.to_string().contains("--dry-run"),
            "the message says why: {err}"
        );
    }

    #[tokio::test]
    async fn queue_add_writes_one_row_and_re_enqueueing_writes_nothing() {
        let fx = Fixture::new().await;
        fx.paper("p-real", Some("10.1101/real"));
        let queued = queue_add(&fx.db, &["p-real".to_string()]).expect("enqueue");
        assert_eq!(queued.enqueued, vec!["p-real"]);
        let after = fx.state_rows();
        let again = queue_add(&fx.db, &["p-real".to_string()]).expect("re-enqueue");
        assert!(again.enqueued.is_empty());
        assert_eq!(again.already_queued, vec!["p-real"]);
        assert_eq!(fx.state_rows(), after, "a re-enqueue writes nothing");
    }
}
