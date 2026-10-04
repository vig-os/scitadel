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

    tracing::info!(
        paper_id = %work.paper_id,
        status = %work.status,
        publisher = ?work.publisher,
        "acquire: walking the ladder"
    );

    match downloader.download_paper(&paper, papers_dir).await {
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
fn paper_of(db: &Database, id: &str) -> Result<Option<Paper>, scitadel_core::error::CoreError> {
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
    use wiremock::matchers::{method, path};
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

        /// The three legacy `papers` columns a dual-written download fills.
        fn legacy_columns(&self) -> Vec<String> {
            let conn = self.db.conn().expect("conn");
            let mut stmt = conn
                .prepare(
                    "SELECT id, local_path, download_status, last_attempt_at FROM papers
                     ORDER BY id",
                )
                .expect("prepare");
            let rows = stmt
                .query_map([], |r| {
                    Ok(format!(
                        "{:?}|{:?}|{:?}|{:?}",
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
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
    // 2. `--dry-run`.
    // =====================================================================

    /// A dry run writes nothing and calls nothing.
    ///
    /// "Writes nothing" is checked as whole-table equality — every column of
    /// `acquisition_state`, every artefact row, and the three legacy `papers`
    /// columns a dual-written download fills — because a count would miss an
    /// in-place update, which is the shape an idempotence bug takes.
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
        let legacy = fx.legacy_columns();

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
        assert_eq!(fx.legacy_columns(), legacy, "no legacy column moved");
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
