//! ADR-007 §3, last paragraph: the `paper_identity_checks` writer and
//! reader.
//!
//! > **Identity** is checked twice, and a mismatch blocks filing anything under
//! > that DOI.
//!
//! Migration 013 gave that sentence a table. It had **no writer anywhere in the
//! workspace** — the same gap that blocked the `publisher` / `next_attempt_at`
//! columns in the previous slice — so this module is what makes the table mean
//! something. The matching itself is `scitadel_adapters::identity`; this side
//! owns the row shape, the write-once rule and the reading.
//!
//! ## One row per `(paper_id, phase)`, not a log
//!
//! `paper_identity_checks` has an autoincrement `id` and **no UNIQUE key**, so
//! "upsert" has to be written out. The upsert key is `(paper_id, phase)` — the
//! work, and the moment in the fetch at which the question was asked — and
//! there are two reasons that is the right key rather than "append and read the
//! newest":
//!
//! 1. [`latest_identity_check`] is the reader every caller wants ("is this
//!    work's identity still disputed?"), and a one-row-per-phase table makes
//!    that answer *structural* rather than an `ORDER BY … LIMIT 1` that a
//!    future writer could get wrong.
//! 2. The override rule below is only expressible on an upsert. "Never
//!    overwrite an `overridden` row" is vacuous against an append-only log: a
//!    newer machine row would simply supersede the human's, and the newest row
//!    would win by construction — which is precisely the
//!    latest-says-what-I-say failure #253 exists to end.
//!
//! ## Why `overridden` is write-once
//!
//! A person's ruling is the one input to this system that cannot be
//! re-derived, and a machine re-asking the question it was told the answer to is
//! not a harmless duplicate — it is a re-litigation. Two consequences, both
//! deliberate:
//!
//! - [`write_identity_check`] **refuses** the write outright rather than
//!   updating the row in place. It does not record the machine check's score or
//!   titles, and that evidence is genuinely lost. The alternative (keeping
//!   `status = 'overridden'` and refreshing the evidence columns) would mean
//!   the row's status and its timestamps disagree about who last touched it,
//!   and "the newest row is the verdict" would silently become "the newest
//!   row's *status* is the verdict, whatever else it says".
//! - A refusal is **not** an error. [`IdentityWrite::PreservedOverride`] is a
//!   normal outcome the caller logs; treating it as a failure would make a
//!   human's decision look like a broken database.
//!
//! The override covers **both** phases, because a person overturning "is this
//! the work I asked for?" is answering the question once, not once per fetch
//! step — and a flag that took a phase argument would be a flag people forget.
//!
//! ## What `unverified` is not
//!
//! `unverified` means *we could not check*, never *the check passed*. It is the
//! only status a caller may read as "carry on", and only because
//! "we could not check" is not evidence of anything (see
//! [`IdentityStatus::is_machine`]). `overridden` is the second status that does
//! not block, and it carries a reason a person wrote.
//!
//! ## Why the attempts row lives here
//!
//! [`record_attempt`] writes `acquisition_attempts`, which — like this table —
//! had no writer in the workspace. It is here rather than in `artefacts.rs`
//! because its only caller in this slice is the identity gate: "we fetched
//! something and refused to file it" is not an artefact fact, it is a fact about
//! a *refused* fetch, and the two rows it writes (the evidence and the outcome)
//! are one story. A general attempt writer is where S2's remaining routes will
//! add their rows.

use chrono::Utc;
use rusqlite::params;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

use crate::error::DbError;
use crate::sqlite::artefacts::{
    FULLTEXT_LOCATOR, StateRow, read_acquisition_state, write_acquisition_states_in,
};

/// The `phase` vocabulary migration 013's CHECK allows.
///
/// Pinned against the live schema by
/// `the_phase_source_and_status_vocabularies_are_exactly_the_schema_check`,
/// in both directions, the same way `coverage.rs` pins `acquisition_state`'s.
pub const ALL_PHASES: [&str; 2] = ["pre_fetch", "post_fetch"];

/// The `source` vocabulary: the four spellings migration 013's column comment
/// documents, plus `human`.
///
/// **The schema does not constrain this column** — `phase` and `status` carry
/// CHECK constraints and `source` is a bare `TEXT NOT NULL` with the vocabulary
/// in a comment. That is why the two lists are pinned differently (the schema
/// test asserts rejections for `phase` and `status` and only counts rows for
/// `source`), and it is why `human` is additive rather than an overwrite: an
/// override is a row in this table, `source` is `NOT NULL`, and the source of a
/// person's ruling is the person. Reusing one of the four machine spellings
/// would have made an override look like OpenAlex said so.
///
/// `crossref` and `datacite` are unreachable this slice — there is no Crossref
/// or DataCite adapter in the workspace, and building one is S3's job — but
/// they are declared here so the writer that does land writes a spelling from
/// one list rather than from a string it invented.
pub const ALL_SOURCES: [&str; 5] = ["openalex", "crossref", "datacite", "served_page", "human"];

/// The `status` vocabulary migration 013's CHECK allows.
pub const ALL_STATUSES: [&str; 4] = ["ok", "mismatch", "unverified", "overridden"];

/// Which side of the fetch a check was taken on (ADR-007 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityPhase {
    /// The expected title (the stored paper) against the resolved one, before
    /// anything is fetched.
    PreFetch,
    /// The served page's or PDF's own title against the metadata's, after the
    /// bytes arrived and before anything is filed.
    PostFetch,
}

impl IdentityPhase {
    /// The column value.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::PreFetch => "pre_fetch",
            Self::PostFetch => "post_fetch",
        }
    }

    /// Every phase, so a caller (and a test) cannot miss one.
    pub const ALL: [Self; 2] = [Self::PreFetch, Self::PostFetch];
}

impl std::fmt::Display for IdentityPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Where a check's answer came from — migration 013's `source` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySource {
    /// A metadata registry's answer about the work.
    OpenAlex,
    /// Crossref. No adapter yet (S3).
    Crossref,
    /// DataCite. No adapter yet (S3).
    DataCite,
    /// The bytes themselves: a served page's or PDF's own title.
    ServedPage,
    /// A person, overriding the machine (`override_identity`).
    ///
    /// Not one of migration 013's four documented spellings, and not an
    /// overwrite of one: the column is `NOT NULL` and unconstrained, so an
    /// override row needs *some* source and the honest answer is the person.
    Human,
}

impl IdentitySource {
    /// The column value.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAlex => "openalex",
            Self::Crossref => "crossref",
            Self::DataCite => "datacite",
            Self::ServedPage => "served_page",
            Self::Human => "human",
        }
    }

    /// Every source, so the vocabulary test can be exhaustive.
    pub const ALL: [Self; 5] = [
        Self::OpenAlex,
        Self::Crossref,
        Self::DataCite,
        Self::ServedPage,
        Self::Human,
    ];
}

impl std::fmt::Display for IdentitySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// What a check concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityStatus {
    /// The two sides agree, corroborated by a year or a first author.
    Ok,
    /// They do not: **nothing may be filed under this DOI until a person says
    /// otherwise.**
    Mismatch,
    /// We could not check — a title we could not read, or no year and no first
    /// author to corroborate a title that only half matched. **Not a pass**,
    /// and not a block either; see [`IdentityStatus::is_machine`].
    Unverified,
    /// A person overturned the machine's verdict. Write-once; see the module
    /// docs.
    Overridden,
}

impl IdentityStatus {
    /// The column value.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Mismatch => "mismatch",
            Self::Unverified => "unverified",
            Self::Overridden => "overridden",
        }
    }

    /// Every status, so the vocabulary test can be exhaustive.
    pub const ALL: [Self; 4] = [Self::Ok, Self::Mismatch, Self::Unverified, Self::Overridden];

    /// Is this a machine verdict, as opposed to a person's ruling?
    ///
    /// The distinction [`write_identity_check`] enforces: an `overridden` row
    /// is write-once, and every other status is not.
    #[must_use]
    pub fn is_machine(self) -> bool {
        !self.is_overridden()
    }

    /// Did a person overrule the machine here?
    #[must_use]
    pub fn is_overridden(self) -> bool {
        matches!(self, Self::Overridden)
    }
}

impl std::fmt::Display for IdentityStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// One identity check to record.
///
/// `checked_at` is taken from the caller rather than from `Utc::now()` inside
/// this module so the same rule that lets every other writer in this crate be
/// idempotent holds here too: a re-run that reaches the same verdict with the
/// same evidence writes nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityCheckWrite {
    pub paper_id: String,
    /// RFC 3339 UTC.
    pub checked_at: String,
    pub phase: IdentityPhase,
    pub source: IdentitySource,
    /// The title we expected — the stored paper's, or the registry's for a
    /// post-fetch check. `None` when the side had no title at all, which is
    /// what makes the check `unverified` rather than `ok`.
    pub expected_title: Option<String>,
    /// The title we got: the resolved metadata's, or the served page's own.
    pub resolved_title: Option<String>,
    /// The similarity score, when one was computed. `None` for a check that
    /// never got as far as comparing (no title on either side).
    pub score: Option<f64>,
    pub status: IdentityStatus,
    /// The DOI this work had before it was fixed, when the fix happened
    /// alongside the check (ADR-007 §3: "`doi_corrected_from` is recorded
    /// whenever a DOI is fixed").
    pub doi_corrected_from: Option<String>,
}

/// What a write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityWrite {
    /// The row was inserted or updated, and this is its `id`.
    Written { id: i64 },
    /// A person had already overruled this `(paper_id, phase)`, so the machine's
    /// verdict was **not** recorded and their reason is returned instead.
    ///
    /// A normal outcome, not an error. The caller logs it; treating it as a
    /// failure would make a human decision look like a broken database.
    PreservedOverride { reason: String },
}

/// One `paper_identity_checks` row as stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IdentityCheckRow {
    pub id: i64,
    pub paper_id: String,
    pub checked_at: String,
    pub phase: IdentityPhase,
    pub source: IdentitySource,
    pub expected_title: Option<String>,
    pub resolved_title: Option<String>,
    pub score: Option<f64>,
    pub status: IdentityStatus,
    pub override_reason: Option<String>,
    pub doi_corrected_from: Option<String>,
}

/// One work's standing identity override, as `coverage` / `action_list` show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityOverride {
    pub paper_id: String,
    /// Every phase this override covers — both, always.
    pub phases: Vec<IdentityPhase>,
    /// The person's own words. Required, never defaulted: an override nobody
    /// can read the reason for is indistinguishable from a bug.
    pub reason: String,
    /// When the override was recorded (RFC 3339 UTC).
    pub recorded_at: String,
}

/// What [`override_identity`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityOverrideReport {
    pub paper_id: String,
    pub phases: Vec<IdentityPhase>,
    pub reason: String,
    /// The want row's status before the override moved it, when it did.
    ///
    /// `identity_mismatch` is ADR-007 §2's human-action status, so a work
    /// blocked by a mismatch stays in `action_list` asking to be checked. Once
    /// a person has checked it, leaving that row alone would keep asking them
    /// again on every run — so the override returns the want to the `acquire`
    /// queue, where it can actually be fetched.
    pub gap_status_before: Option<String>,
    /// The want row's status after, when there was one to move.
    pub gap_status_after: Option<String>,
}

/// Anything that can go wrong around an identity check.
///
/// A distinct type rather than more [`DbError`] variants because two of the
/// three cases are bad **input** (no reason given, no such work), and their
/// messages have to tell a person what to type next — which a "migration
/// error" prefix would bury.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error(transparent)]
    Db(#[from] DbError),
    /// The escape hatch without its evidence.
    ///
    /// Refused rather than defaulted, because a stored empty reason is
    /// indistinguishable from an override nobody explained, which is the one
    /// thing this flag exists to prevent.
    #[error(
        "an identity override must carry a reason: pass the reason a reader of the \
         database will understand in six months, because `paper_identity_checks.\
         override_reason` is the only record that a person looked at this work and \
         decided the mismatch was not one"
    )]
    ReasonRequired,
    /// No `papers` row. With `PRAGMA foreign_keys` on, the insert would refuse
    /// with a foreign-key error that names a constraint rather than the paper
    /// the caller asked for.
    #[error("no work named {0}; an identity check is a statement about a paper row")]
    NoSuchWork(String),
}

/// Record one identity check for `(paper_id, phase)`.
///
/// Upserts on `(paper_id, phase)` inside one `BEGIN IMMEDIATE`, and **refuses**
/// to touch a row a person has marked `overridden` — see the module docs for
/// why the evidence is sacrificed rather than merged.
///
/// The `WHERE` guard is an identity comparison, not "always true": a re-run that
/// reaches the same verdict with the same evidence writes no row and moves no
/// `checked_at`, which is what keeps "a second run makes no writes" true of
/// this table as well as of `artefacts`.
pub fn write_identity_check(
    conn: &mut Connection,
    row: &IdentityCheckWrite,
) -> Result<IdentityWrite, DbError> {
    // A person may not record an override through this path, and a machine may
    // not clear one: the status is the row's authority, and letting a caller
    // pick it freely is how `overridden` would stop meaning "a person said so".
    if !row.status.is_machine() {
        return Err(DbError::Migration(format!(
            "an identity check written through this path must be a machine verdict \
             (ok | mismatch | unverified); {:?} is recorded by `override_identity`, \
             which requires a reason",
            row.status
        )));
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing = existing_check(&tx, &row.paper_id, row.phase)?;

    if let Some(overridden) = existing.as_ref().filter(|e| e.status.is_overridden()) {
        let reason: String = tx.query_row(
            "SELECT COALESCE(override_reason, '') FROM paper_identity_checks WHERE id = ?1",
            params![overridden.id],
            |r| r.get(0),
        )?;
        // Nothing was written, so the transaction has no effect either way;
        // rolling back is the cheaper of the two and makes it obvious that the
        // refusal is not a write that happened to change no columns.
        drop(tx);
        return Ok(IdentityWrite::PreservedOverride { reason });
    }
    let existing = existing.map(|existing| existing.id);

    let written = if let Some(id) = existing {
        tx.execute(
            "UPDATE paper_identity_checks
                    SET checked_at = ?1, source = ?2, expected_title = ?3,
                        resolved_title = ?4, score = ?5, status = ?6,
                        doi_corrected_from = ?7
                  WHERE id = ?8
                    AND (checked_at IS NOT ?1
                      OR source IS NOT ?2
                      OR expected_title IS NOT ?3
                      OR resolved_title IS NOT ?4
                      OR score IS NOT ?5
                      OR status IS NOT ?6
                      OR doi_corrected_from IS NOT ?7)",
            params![
                row.checked_at,
                row.source.label(),
                row.expected_title,
                row.resolved_title,
                row.score,
                row.status.label(),
                row.doi_corrected_from,
                id,
            ],
        )?
    } else {
        tx.execute(
            "INSERT INTO paper_identity_checks
                 (paper_id, checked_at, phase, source, expected_title, resolved_title,
                  score, status, doi_corrected_from)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                row.paper_id,
                row.checked_at,
                row.phase.label(),
                row.source.label(),
                row.expected_title,
                row.resolved_title,
                row.score,
                row.status.label(),
                row.doi_corrected_from,
            ],
        )?;
        1
    };
    let id = existing.unwrap_or_else(|| tx.last_insert_rowid());
    tx.commit()?;
    if written == 0 {
        tracing::debug!(
            paper_id = %row.paper_id,
            phase = %row.phase,
            "identity check unchanged; nothing written"
        );
    }
    Ok(IdentityWrite::Written { id })
}

/// The check in force for `(paper_id, phase)`, if there is one.
///
/// `None` means **no check has ever been taken**, which is not the same as
/// `unverified`: it means the caller has not looked yet.
pub fn latest_identity_check(
    conn: &Connection,
    paper_id: &str,
    phase: IdentityPhase,
) -> Result<Option<IdentityCheckRow>, DbError> {
    conn.query_row(
        "SELECT id, paper_id, checked_at, phase, source, expected_title, resolved_title,
                score, status, override_reason, doi_corrected_from
           FROM paper_identity_checks
          WHERE paper_id = ?1 AND phase = ?2",
        params![paper_id, phase.label()],
        row_to_identity_check,
    )
    .optional()
    .map_err(DbError::from)
}

/// Every work a person has overruled, for `coverage` and `action_list`.
///
/// One row per work, not per `(work, phase)`: the override always covers both
/// phases, and a report that printed each work twice would read as two
/// decisions.
pub fn identity_overrides(conn: &Connection) -> Result<Vec<IdentityOverride>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT paper_id, phase, COALESCE(override_reason, ''), checked_at
           FROM paper_identity_checks
          WHERE status = 'overridden'
          ORDER BY paper_id, phase",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out: Vec<IdentityOverride> = Vec::new();
    for row in rows {
        let (paper_id, phase, reason, recorded_at) = row?;
        if let Some(existing) = out.iter_mut().find(|o| o.paper_id == paper_id) {
            existing.phases.push(phase_of(&phase)?);
            continue;
        }
        out.push(IdentityOverride {
            paper_id,
            phases: vec![phase_of(&phase)?],
            reason,
            recorded_at,
        });
    }
    Ok(out)
}

/// Record a person's ruling: this work's identity is settled, whatever the
/// machine says.
///
/// Writes one `overridden` row per phase, and returns the want row's status
/// transition when it had one — see [`IdentityOverrideReport`].
///
/// A second override for the same work **updates** the row rather than
/// inserting another: unlike a machine verdict, a person is allowed to change
/// their mind, and the newest reason is the one that stands. That asymmetry is
/// the whole point of the write-once rule above being one-directional.
///
/// # Errors
///
/// [`IdentityError::ReasonRequired`] for a blank reason,
/// [`IdentityError::NoSuchWork`] when there is no `papers` row, and
/// [`IdentityError::Db`] for anything SQLite raises.
pub fn override_identity(
    conn: &mut Connection,
    paper_id: &str,
    reason: &str,
) -> Result<IdentityOverrideReport, IdentityError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(IdentityError::ReasonRequired);
    }
    let now = Utc::now().to_rfc3339();
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM papers WHERE id = ?1)",
            params![paper_id],
            |r| r.get(0),
        )
        .map_err(DbError::from)?;
    if !exists {
        return Err(IdentityError::NoSuchWork(paper_id.to_string()));
    }

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(DbError::from)?;
    for phase in IdentityPhase::ALL {
        // The upsert key `(paper_id, phase)` has no UNIQUE index, so
        // `ON CONFLICT` has nothing to bite on and the update has to be
        // written out. Only the status, the reason, the source and the timestamp
        // move: a machine check's evidence is never overwritten by a person any
        // more than the other way round.
        match existing_check(&tx, paper_id, phase)? {
            Some(existing) => {
                tx.execute(
                    "UPDATE paper_identity_checks
                        SET status = 'overridden', override_reason = ?1, checked_at = ?2,
                            source = ?3
                      WHERE id = ?4",
                    params![reason, now, IdentitySource::Human.label(), existing.id],
                )
                .map_err(DbError::from)?;
            }
            None => {
                tx.execute(
                    "INSERT INTO paper_identity_checks
                         (paper_id, checked_at, phase, source, status, override_reason)
                     VALUES (?1, ?2, ?3, ?4, 'overridden', ?5)",
                    params![
                        paper_id,
                        now,
                        phase.label(),
                        IdentitySource::Human.label(),
                        reason
                    ],
                )
                .map_err(DbError::from)?;
            }
        }
    }

    // Return a work whose mismatch has now been ruled on to the `acquire`
    // queue. The row is read, not constructed, so `drop_path`, `hint_url` and
    // `publisher` cannot be lost on the way through.
    let gap: Option<StateRow> =
        read_acquisition_state(&tx, paper_id, GAP_FULLTEXT_KIND, FULLTEXT_LOCATOR)?;
    let mut before = None;
    let mut after = None;
    if let Some(row) = gap
        && row.status == "identity_mismatch"
    {
        before = Some(row.status.clone());
        let mut moved = row.to_write();
        moved.status = "pending".to_string();
        moved.reason = Some(format!(
            "identity mismatch overturned by a person on {now}: {}",
            row.reason.as_deref().unwrap_or("(no recorded reason)")
        ));
        // Due now: `next_attempt_at` is the queue's own column and this row no
        // longer carries a schedule a campaign chose — a blocked work has
        // nothing to be back off from.
        moved.next_attempt_at = None;
        moved.updated_at.clone_from(&now);
        write_acquisition_states_in(&tx, std::slice::from_ref(&moved))?;
        after = Some(moved.status);
    }
    tx.commit().map_err(DbError::from)?;

    Ok(IdentityOverrideReport {
        paper_id: paper_id.to_string(),
        phases: IdentityPhase::ALL.to_vec(),
        reason: reason.to_string(),
        gap_status_before: before,
        gap_status_after: after,
    })
}

/// One row of `acquisition_attempts`: what a fetch did, including when it
/// deliberately filed nothing.
///
/// The table had no writer in the workspace before this slice. Only the
/// `identity_mismatch` outcome is reachable today, and it is the one that has
/// to be reachable: a fetch refused by the identity gate has to leave a row
/// saying so, or "we did not file it" and "we never fetched it" are the same
/// fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptWrite {
    /// `None` only when the work row was deleted mid-fetch — the column is
    /// `ON DELETE SET NULL`, because an audit trail that vanishes with its
    /// subject is not an audit trail.
    pub paper_id: Option<String>,
    pub kind: String,
    pub locator: String,
    /// `artefacts.route` — which ladder step was tried.
    pub route: String,
    /// The fetch's own id, when one has a bucket to name. `None` for the
    /// refusal paths, which never reached a ledger.
    pub bucket: Option<String>,
    /// RFC 3339 UTC.
    pub started_at: String,
    /// One of migration 013's `outcome` spellings:
    /// `ok|http_error|login_redirect|too_large|bad_magic|identity_mismatch|denied|cancelled`.
    pub outcome: String,
    pub http_status: Option<i64>,
    pub detail: Option<String>,
}

/// The `acquisition_state.kind` an identity override can return to the queue.
///
/// The full-text want, because that is the only kind a fetch closes (ADR-007
/// §3's routes are all full-text routes), so it is the only gap a mismatch can
/// have blocked. Spelled here rather than imported from
/// `scitadel_adapters::acquire`, which this crate must not depend on.
const GAP_FULLTEXT_KIND: &str = "fulltext";

/// The `acquisition_attempts.outcome` vocabulary migration 013 documents.
pub const ALL_ATTEMPT_OUTCOMES: [&str; 8] = [
    "ok",
    "http_error",
    "login_redirect",
    "too_large",
    "bad_magic",
    "identity_mismatch",
    "denied",
    "cancelled",
];

/// Insert one attempt row, returning its `id`.
pub fn record_attempt(conn: &Connection, attempt: &AttemptWrite) -> Result<i64, DbError> {
    if !ALL_ATTEMPT_OUTCOMES.contains(&attempt.outcome.as_str()) {
        return Err(DbError::Migration(format!(
            "{:?} is not an `acquisition_attempts.outcome` this build knows; valid: {}",
            attempt.outcome,
            ALL_ATTEMPT_OUTCOMES.join(", ")
        )));
    }
    conn.execute(
        "INSERT INTO acquisition_attempts
             (paper_id, kind, locator, route, bucket, started_at, outcome, http_status, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            attempt.paper_id,
            attempt.kind,
            attempt.locator,
            attempt.route,
            attempt.bucket,
            attempt.started_at,
            attempt.outcome,
            attempt.http_status,
            attempt.detail,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fix a work's DOI and record where it came from (ADR-007 §3).
///
/// One transaction: the corrected DOI on `papers`, and the
/// `paper_identity_checks` row carrying `doi_corrected_from` beside it. A DOI
/// fix without the `doi_corrected_from` would leave the schema's only column
/// for "we changed this work's identity" permanently unused, and a corrected
/// DOI with no record of the old one is how a work ends up filed twice.
///
/// Callers in this slice: none. S3's resolve-then-rank pass is what produces a
/// corrected DOI, and it is where this is meant to be called; it is here, and
/// tested, so that when it lands the recording cannot be forgotten.
///
/// The `overridden` guard is the same one [`write_identity_check`] applies: a
/// person who has ruled on this work's identity is not overruled by a later
/// machine fix. In that case the DOI is still corrected — the paper is
/// demonstrably that work — but the correction is not recorded as a check.
pub fn correct_doi(
    conn: &mut Connection,
    paper_id: &str,
    from: &str,
    to: &str,
) -> Result<bool, DbError> {
    let updated = conn.execute(
        "UPDATE papers SET doi = ?1, updated_at = ?2 WHERE id = ?3",
        params![to, Utc::now().to_rfc3339(), paper_id],
    )?;
    if updated != 1 {
        return Err(DbError::Migration(format!(
            "no papers row for {paper_id}, so its DOI could not be corrected"
        )));
    }
    let write = IdentityCheckWrite {
        paper_id: paper_id.to_string(),
        checked_at: Utc::now().to_rfc3339(),
        phase: IdentityPhase::PreFetch,
        source: IdentitySource::OpenAlex,
        expected_title: None,
        resolved_title: None,
        score: None,
        status: IdentityStatus::Ok,
        doi_corrected_from: Some(from.to_string()),
    };
    Ok(matches!(
        write_identity_check(conn, &write)?,
        IdentityWrite::Written { .. }
    ))
}

/// `papers.osti_id` for `paper_id` — the identifier ADR-007 §3 step 3 fetches
/// DOE national-lab reports by.
///
/// A targeted reader rather than a `Paper` field on purpose. `Paper` is written
/// back wholesale by `save()`, so a `Paper` carrying `osti_id: None` for a work
/// that *has* one would silently null the column on the next write — the
/// migration's own ADR note ("`pmcid` and `osti_id` join the `Paper` model and
/// row mapping in S1") predates the hazard, and this slice is where the hazard
/// becomes real.
pub fn read_osti_id(conn: &Connection, paper_id: &str) -> Result<Option<String>, DbError> {
    conn.query_row(
        "SELECT osti_id FROM papers WHERE id = ?1",
        params![paper_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    // A work with no row reads as "no osti_id", which is what the ladder's
    // `Skipped` arm wants; a broken database is not that.
    .map(|row| row.flatten())
    .map_err(DbError::from)
}

/// Set `papers.osti_id` for `paper_id`, returning whether a row was updated.
///
/// The only writer today is this crate's own tests and `scitadel import-flat`;
/// S3's metadata pass is the production caller. It is a single-column update on
/// purpose: nothing that saves a whole `Paper` can clobber it.
pub fn write_osti_id(conn: &Connection, paper_id: &str, osti_id: &str) -> Result<bool, DbError> {
    let updated = conn.execute(
        "UPDATE papers SET osti_id = ?1 WHERE id = ?2",
        params![osti_id.trim(), paper_id],
    )?;
    Ok(updated == 1)
}

fn row_to_identity_check(row: &rusqlite::Row) -> rusqlite::Result<IdentityCheckRow> {
    let phase: String = row.get("phase")?;
    let source: String = row.get("source")?;
    let status: String = row.get("status")?;
    let score: Option<f64> = row.get("score")?;
    Ok(IdentityCheckRow {
        id: row.get("id")?,
        paper_id: row.get("paper_id")?,
        checked_at: row.get("checked_at")?,
        phase: phase_of(&phase)?,
        source: source_of(&source)?,
        expected_title: row.get("expected_title")?,
        resolved_title: row.get("resolved_title")?,
        score,
        status: status_of(&status)?,
        override_reason: row.get("override_reason")?,
        doi_corrected_from: row.get("doi_corrected_from")?,
    })
}

/// The one row the upsert key `(paper_id, phase)` names, if there is one.
///
/// Written out rather than done with `ON CONFLICT` because the table has no
/// UNIQUE index on `(paper_id, phase)` — migration 013 gives it an
/// autoincrement `id` and nothing else. Two writers (a machine check and a
/// person's override) share this lookup so the "upsert" they both mean cannot
/// be spelled two ways.
fn existing_check(
    conn: &Connection,
    paper_id: &str,
    phase: IdentityPhase,
) -> Result<Option<ExistingCheck>, DbError> {
    conn.query_row(
        "SELECT id, status FROM paper_identity_checks
          WHERE paper_id = ?1 AND phase = ?2",
        params![paper_id, phase.label()],
        |r| {
            let status: String = r.get(1)?;
            Ok(ExistingCheck {
                id: r.get(0)?,
                status: status_of(&status)?,
            })
        },
    )
    .optional()
    .map_err(DbError::from)
}

/// The identity and verdict of a check row that already exists.
struct ExistingCheck {
    id: i64,
    status: IdentityStatus,
}

/// A `phase` string migration 013's CHECK would have refused.
fn phase_of(label: &str) -> rusqlite::Result<IdentityPhase> {
    match label {
        "pre_fetch" => Ok(IdentityPhase::PreFetch),
        "post_fetch" => Ok(IdentityPhase::PostFetch),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            other.len(),
            rusqlite::types::Type::Text,
            unknown("phase", other, &ALL_PHASES).into(),
        )),
    }
}

fn source_of(label: &str) -> rusqlite::Result<IdentitySource> {
    match label {
        "openalex" => Ok(IdentitySource::OpenAlex),
        "crossref" => Ok(IdentitySource::Crossref),
        "datacite" => Ok(IdentitySource::DataCite),
        "served_page" => Ok(IdentitySource::ServedPage),
        "human" => Ok(IdentitySource::Human),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            other.len(),
            rusqlite::types::Type::Text,
            unknown("source", other, &ALL_SOURCES).into(),
        )),
    }
}

fn status_of(label: &str) -> rusqlite::Result<IdentityStatus> {
    match label {
        "ok" => Ok(IdentityStatus::Ok),
        "mismatch" => Ok(IdentityStatus::Mismatch),
        "unverified" => Ok(IdentityStatus::Unverified),
        "overridden" => Ok(IdentityStatus::Overridden),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            other.len(),
            rusqlite::types::Type::Text,
            unknown("status", other, &ALL_STATUSES).into(),
        )),
    }
}

fn unknown(column: &str, value: &str, vocabulary: &[&str]) -> String {
    format!(
        "{value:?} is not a paper_identity_checks.{column} this build knows; valid: {}",
        vocabulary.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use scitadel_core::models::Paper;
    use scitadel_core::ports::PaperRepository as _;
    use tempfile::TempDir;

    const NOW: &str = "2026-03-04T05:06:07+00:00";
    const LATER: &str = "2026-04-05T06:07:08+00:00";

    /// A migrated database. The `TempDir` is held for the fixture's lifetime
    /// because the pool opens connections lazily.
    struct Fx {
        _dir: TempDir,
        db: crate::sqlite::Database,
    }

    impl Fx {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = crate::sqlite::Database::open(&dir.path().join("scitadel.db")).unwrap();
            db.migrate().unwrap();
            Self { _dir: dir, db }
        }

        fn paper(&self, id: &str) {
            let (papers, _, _, _, _) = self.db.repositories();
            let mut paper = Paper::new(format!("Paper {id}"));
            // `Paper::new` mints a random id; these tests address works by name,
            // and `paper_identity_checks.paper_id` is a foreign key into
            // `papers(id)` — so the id has to be the one the test uses.
            paper.id = scitadel_core::models::PaperId::from(id.to_string());
            papers.save(&paper).unwrap();
        }

        fn write(&self, row: &IdentityCheckWrite) -> IdentityWrite {
            let mut conn = self.db.conn().unwrap();
            write_identity_check(&mut conn, row).unwrap()
        }

        fn latest(&self, paper_id: &str, phase: IdentityPhase) -> Option<IdentityCheckRow> {
            let conn = self.db.conn().unwrap();
            latest_identity_check(&conn, paper_id, phase).unwrap()
        }

        /// Every check row, in order — the shape the override tests compare.
        fn rows(&self) -> Vec<(String, String, String, String, Option<String>)> {
            let conn = self.db.conn().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT paper_id, phase, status, checked_at, override_reason
                       FROM paper_identity_checks ORDER BY paper_id, phase",
                )
                .unwrap();
            stmt.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        }

        fn count(&self, sql: &str) -> i64 {
            let conn = self.db.conn().unwrap();
            conn.query_row(sql, [], |r| r.get(0)).unwrap()
        }
    }

    fn check(paper_id: &str, phase: IdentityPhase, status: IdentityStatus) -> IdentityCheckWrite {
        IdentityCheckWrite {
            paper_id: paper_id.to_string(),
            checked_at: NOW.to_string(),
            phase,
            source: IdentitySource::OpenAlex,
            expected_title: Some("expected title".into()),
            resolved_title: Some("a completely different title".into()),
            score: Some(0.31),
            status,
            doi_corrected_from: None,
        }
    }

    // =====================================================================
    // The vocabulary is the schema's, in both directions.
    // =====================================================================

    /// Every label this module can write must satisfy the table's own CHECK,
    /// and the schema must refuse a `phase` or `status` it does not know —
    /// otherwise the rejections below would prove nothing, and a value written
    /// through `write_identity_check` could raise at acquisition time instead
    /// of at compile time.
    ///
    /// `source` is the one column migration 013 leaves unconstrained (its
    /// vocabulary is in a comment, not a CHECK), so it is pinned by
    /// `the_declared_vocabularies_and_the_enums_agree` and by the fact that each
    /// spelling reads back through [`IdentitySource::label`].
    #[test]
    fn the_phase_source_and_status_vocabularies_are_exactly_the_schema_check() {
        let fx = Fx::new();
        fx.paper("p-1");
        for phase in IdentityPhase::ALL {
            for source in IdentitySource::ALL {
                for status in IdentityStatus::ALL
                    .into_iter()
                    .filter(|status| status.is_machine())
                {
                    fx.write(&IdentityCheckWrite {
                        paper_id: "p-1".into(),
                        checked_at: NOW.into(),
                        phase,
                        source,
                        expected_title: None,
                        resolved_title: None,
                        score: None,
                        status,
                        doi_corrected_from: None,
                    });
                }
            }
        }
        assert_eq!(
            fx.count("SELECT COUNT(*) FROM paper_identity_checks"),
            2,
            "one row per (work, phase) whatever the source or the status"
        );

        let conn = fx.db.conn().unwrap();
        conn.execute(
            "INSERT INTO paper_identity_checks (paper_id, checked_at, phase, source, status)
             VALUES ('p-1', '2026-01-01', 'mid_fetch', 'openalex', 'ok')",
            [],
        )
        .expect_err("the schema must refuse a phase outside the vocabulary");
        conn.execute(
            "INSERT INTO paper_identity_checks (paper_id, checked_at, phase, source, status)
             VALUES ('p-1', '2026-01-01', 'pre_fetch', 'openalex', 'probably_fine')",
            [],
        )
        .expect_err("the schema must refuse a status outside the vocabulary");
        conn.execute(
            "INSERT INTO paper_identity_checks (paper_id, checked_at, phase, source, status)
             VALUES ('p-1', '2026-01-01', 'pre_fetch', NULL, 'ok')",
            [],
        )
        .expect_err("`source` is NOT NULL whatever else about it is unconstrained");
    }

    /// Every label this module declares is one it can also write, in both
    /// directions — the list and the enum cannot drift.
    #[test]
    fn the_declared_vocabularies_and_the_enums_agree() {
        let phases: Vec<&str> = IdentityPhase::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(phases, ALL_PHASES.to_vec());
        let sources: Vec<&str> = IdentitySource::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(sources, ALL_SOURCES.to_vec());
        let statuses: Vec<&str> = IdentityStatus::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(statuses, ALL_STATUSES.to_vec());
        assert!(
            IdentityStatus::Overridden.is_overridden(),
            "the escape hatch's status must be distinguishable from every machine verdict"
        );
        for status in IdentityStatus::ALL
            .into_iter()
            .filter(|status| status.is_machine())
        {
            assert!(!status.is_overridden(), "{status} is a machine verdict");
        }
    }

    // =====================================================================
    // The writer.
    // =====================================================================

    /// One row per `(paper_id, phase)`: a second check of the same phase
    /// updates, and a check of the *other* phase is its own row. Both titles
    /// are kept verbatim — the schema's whole point is being able to see them.
    #[test]
    fn a_check_is_one_row_per_work_and_phase() {
        let fx = Fx::new();
        fx.paper("p-1");
        let first = fx.write(&check(
            "p-1",
            IdentityPhase::PreFetch,
            IdentityStatus::Mismatch,
        ));
        assert!(matches!(first, IdentityWrite::Written { .. }));

        let mut later = check("p-1", IdentityPhase::PreFetch, IdentityStatus::Mismatch);
        later.checked_at = LATER.to_string();
        later.resolved_title = Some("yet another title".into());
        fx.write(&later);
        fx.write(&check(
            "p-1",
            IdentityPhase::PostFetch,
            IdentityStatus::Unverified,
        ));

        assert_eq!(fx.count("SELECT COUNT(*) FROM paper_identity_checks"), 2);
        let pre = fx.latest("p-1", IdentityPhase::PreFetch).unwrap();
        assert_eq!(
            pre.checked_at, LATER,
            "the newest check is the one in force"
        );
        assert_eq!(pre.resolved_title.as_deref(), Some("yet another title"));
        let post = fx.latest("p-1", IdentityPhase::PostFetch).unwrap();
        assert_eq!(post.status, IdentityStatus::Unverified);
        assert_eq!(
            post.expected_title.as_deref(),
            Some("expected title"),
            "both titles are stored so a human can read them"
        );
    }

    /// An unchanged re-run writes nothing at all and moves no timestamp —
    /// `artefacts`' "a second run changes nothing" contract, on this table too.
    #[test]
    fn an_unchanged_check_writes_no_row_and_moves_no_timestamp() {
        let fx = Fx::new();
        fx.paper("p-1");
        fx.write(&check("p-1", IdentityPhase::PreFetch, IdentityStatus::Ok));
        let id = match fx.write(&check("p-1", IdentityPhase::PreFetch, IdentityStatus::Ok)) {
            IdentityWrite::Written { id } => id,
            other @ IdentityWrite::PreservedOverride { .. } => panic!("{other:?}"),
        };
        assert_eq!(fx.count("SELECT COUNT(*) FROM paper_identity_checks"), 1);
        assert_eq!(fx.latest("p-1", IdentityPhase::PreFetch).unwrap().id, id);

        // And the timestamp really is the caller's, not a fresh `now()`.
        assert_eq!(
            fx.latest("p-1", IdentityPhase::PreFetch)
                .unwrap()
                .checked_at,
            NOW
        );
    }

    /// A machine check may not *write* an override. The status is the row's
    /// authority: if a caller could pick `overridden`, it would stop meaning
    /// "a person said so".
    #[test]
    fn the_machine_writer_refuses_to_record_an_override() {
        let fx = Fx::new();
        fx.paper("p-1");
        let mut conn = fx.db.conn().unwrap();
        let err = write_identity_check(
            &mut conn,
            &check("p-1", IdentityPhase::PreFetch, IdentityStatus::Overridden),
        )
        .expect_err("an override must come from `override_identity`, with a reason");
        assert!(err.to_string().contains("override_identity"), "{err}");
    }

    /// #253's escape hatch: a second run must not undo a person's ruling.
    /// The machine's verdict is **not recorded at all** — not merged into the
    /// row behind the override's back.
    #[test]
    fn an_override_survives_a_later_run() {
        let fx = Fx::new();
        fx.paper("p-1");
        fx.write(&check(
            "p-1",
            IdentityPhase::PreFetch,
            IdentityStatus::Mismatch,
        ));
        fx.write(&check(
            "p-1",
            IdentityPhase::PostFetch,
            IdentityStatus::Mismatch,
        ));

        let mut conn = fx.db.conn().unwrap();
        let report = override_identity(
            &mut conn,
            "p-1",
            "the OSTI record and the journal article are the same report; \
             OpenAlex has the pre-2024 title",
        )
        .unwrap();
        assert_eq!(report.phases.len(), 2, "an override covers both phases");
        drop(conn);

        // Two later machine runs, both landing on a verdict that would have
        // blocked the work.
        fx.write(&check(
            "p-1",
            IdentityPhase::PreFetch,
            IdentityStatus::Mismatch,
        ));
        fx.write(&check(
            "p-1",
            IdentityPhase::PostFetch,
            IdentityStatus::Mismatch,
        ));

        let rows = fx.rows();
        assert_eq!(rows.len(), 2, "no new rows: an override is write-once");
        for (_, phase, status, _, reason) in &rows {
            assert_eq!(status, "overridden", "the {phase} verdict stands");
            assert!(
                reason.as_deref().is_some_and(|r| r.contains("same report")),
                "the person's words survive verbatim: {reason:?}"
            );
        }
        let mut conn = fx.db.conn().unwrap();
        let refused = write_identity_check(
            &mut conn,
            &check("p-1", IdentityPhase::PreFetch, IdentityStatus::Mismatch),
        )
        .unwrap();
        assert!(
            matches!(refused, IdentityWrite::PreservedOverride { ref reason } if reason.contains("same report")),
            "the refusal is a normal outcome carrying the reason: {refused:?}"
        );
    }

    /// The override's own writer requires a reason, and a blank one is refused
    /// rather than stored as an empty string — an unexplained override is
    /// indistinguishable from a bug.
    #[test]
    fn an_override_requires_a_reason() {
        let fx = Fx::new();
        fx.paper("p-1");
        let mut conn = fx.db.conn().unwrap();
        for blank in ["", "   ", "\n\t "] {
            let err = override_identity(&mut conn, "p-1", blank)
                .expect_err("a blank reason is not a reason");
            assert!(matches!(err, IdentityError::ReasonRequired), "{err}");
            assert!(err.to_string().contains("must carry a reason"), "{err}");
        }
        assert_eq!(fx.count("SELECT COUNT(*) FROM paper_identity_checks"), 0);

        let err = override_identity(&mut conn, "p-no-such-work", "because")
            .expect_err("there is no such work");
        assert!(matches!(err, IdentityError::NoSuchWork(id) if id == "p-no-such-work"));
    }

    /// A person is allowed to change their mind — the override is write-once
    /// against *machines*, not against a second `--reason`.
    #[test]
    fn a_person_may_replace_their_own_override() {
        let fx = Fx::new();
        fx.paper("p-1");
        let mut conn = fx.db.conn().unwrap();
        override_identity(&mut conn, "p-1", "first, wrong reason").unwrap();
        override_identity(&mut conn, "p-1", "second, checked again").unwrap();
        drop(conn);

        let rows = fx.rows();
        assert_eq!(rows.len(), 2, "still one row per phase");
        for (_, _, status, _, reason) in &rows {
            assert_eq!(status, "overridden");
            assert_eq!(reason.as_deref(), Some("second, checked again"));
        }
    }

    /// Overriding a work that an identity mismatch had blocked returns it to
    /// the `acquire` queue — otherwise the human keeps being asked to check a
    /// DOI they have already checked, and the work is never fetched.
    #[test]
    fn an_override_returns_a_mismatched_gap_to_the_queue() {
        let fx = Fx::new();
        fx.paper("p-1");
        let mut conn = fx.db.conn().unwrap();
        crate::sqlite::write_acquisition_states(
            &mut conn,
            &[crate::sqlite::StateWrite {
                paper_id: "p-1".into(),
                kind: "fulltext".into(),
                locator: crate::sqlite::FULLTEXT_LOCATOR.into(),
                wanted_version: "vor".into(),
                status: "identity_mismatch".into(),
                reason: Some("served title was a different paper".into()),
                publisher: Some("nature".into()),
                hint_url: Some("https://doi.org/10.1/x".into()),
                drop_path: None,
                next_attempt_at: None,
                updated_at: NOW.into(),
            }],
        )
        .unwrap();

        let report = override_identity(&mut conn, "p-1", "checked by hand; same work").unwrap();
        assert_eq!(
            report.gap_status_before.as_deref(),
            Some("identity_mismatch")
        );
        assert_eq!(report.gap_status_after.as_deref(), Some("pending"));

        let row = crate::sqlite::read_acquisition_state(
            &conn,
            "p-1",
            "fulltext",
            crate::sqlite::FULLTEXT_LOCATOR,
        )
        .unwrap()
        .unwrap();
        assert_eq!(row.status, "pending");
        assert!(
            row.reason
                .as_deref()
                .is_some_and(|r| r.contains("overturned by a person")),
            "the reason says why the work is back in the queue: {:?}",
            row.reason
        );
        assert_eq!(
            row.publisher.as_deref(),
            Some("nature"),
            "a read-modify-write keeps every other column"
        );
        assert_eq!(row.hint_url.as_deref(), Some("https://doi.org/10.1/x"));
    }

    /// A work with no mismatch is overridden without touching the queue — the
    /// override is a statement about identity, not about what to fetch.
    #[test]
    fn an_override_of_an_unblocked_work_touches_no_gap() {
        let fx = Fx::new();
        fx.paper("p-1");
        let mut conn = fx.db.conn().unwrap();
        let report =
            override_identity(&mut conn, "p-1", "pre-empting a known index quirk").unwrap();
        assert_eq!(report.gap_status_before, None);
        assert_eq!(report.gap_status_after, None);
    }

    /// The overrides a report has to show, one row per **work** however many
    /// phases it covers — a report that printed the work twice would read as
    /// two decisions.
    #[test]
    fn overrides_are_listed_once_per_work_for_the_reports() {
        let fx = Fx::new();
        fx.paper("p-1");
        fx.paper("p-2");
        let mut conn = fx.db.conn().unwrap();
        override_identity(&mut conn, "p-1", "same report, two titles").unwrap();
        override_identity(&mut conn, "p-2", "OpenAlex has the old title").unwrap();
        // A machine check on p-2 must not remove it from the list.
        drop(conn);
        fx.write(&check(
            "p-2",
            IdentityPhase::PreFetch,
            IdentityStatus::Mismatch,
        ));

        let conn = fx.db.conn().unwrap();
        let list = identity_overrides(&conn).unwrap();
        assert_eq!(list.len(), 2, "one entry per work: {list:?}");
        assert_eq!(list[0].paper_id, "p-1");
        assert_eq!(list[0].phases.len(), 2, "both phases, listed once");
        assert_eq!(list[1].reason, "OpenAlex has the old title");
    }

    // =====================================================================
    // The attempt row, and the DOI correction.
    // =====================================================================

    /// The identity gate's own audit row: a fetch that deliberately filed
    /// nothing leaves a row saying so.
    #[test]
    fn a_refused_fetch_is_recorded_as_an_attempt() {
        let fx = Fx::new();
        fx.paper("p-1");
        let conn = fx.db.conn().unwrap();
        record_attempt(
            &conn,
            &AttemptWrite {
                paper_id: Some("p-1".into()),
                kind: "fulltext".into(),
                locator: String::new(),
                route: "osti".into(),
                bucket: Some("osti.gov".into()),
                started_at: NOW.into(),
                outcome: "identity_mismatch".into(),
                http_status: Some(200),
                detail: Some("expected \"A\"; served \"B\"; similarity 0.31".into()),
            },
        )
        .unwrap();

        let (outcome, detail, http): (String, String, i64) = conn
            .query_row(
                "SELECT outcome, detail, http_status FROM acquisition_attempts",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(outcome, "identity_mismatch");
        assert!(detail.contains("similarity 0.31"), "{detail}");
        assert_eq!(http, 200);
    }

    /// An outcome outside migration 013's documented list is refused by name,
    /// rather than written and later found by a reader.
    #[test]
    fn an_unknown_attempt_outcome_is_refused_by_name() {
        let fx = Fx::new();
        fx.paper("p-1");
        let conn = fx.db.conn().unwrap();
        let err = record_attempt(
            &conn,
            &AttemptWrite {
                paper_id: Some("p-1".into()),
                kind: "fulltext".into(),
                locator: String::new(),
                route: "osti".into(),
                bucket: None,
                started_at: NOW.into(),
                outcome: "identity_mismatched".into(),
                http_status: None,
                detail: None,
            },
        )
        .expect_err("a typo must not reach the column");
        assert!(err.to_string().contains("identity_mismatched"), "{err}");
        assert_eq!(fx.count("SELECT COUNT(*) FROM acquisition_attempts"), 0);
    }

    /// ADR-007 §3: "`doi_corrected_from` is recorded whenever a DOI is fixed."
    /// Both halves in one transaction, and the old DOI survives on the row.
    #[test]
    fn a_corrected_doi_records_where_it_came_from() {
        let fx = Fx::new();
        fx.paper("p-1");
        let mut conn = fx.db.conn().unwrap();
        assert!(
            correct_doi(
                &mut conn,
                "p-1",
                "10.9999/wrong.1",
                "10.1038/s41586-020-2649-2"
            )
            .unwrap()
        );

        let doi: String = conn
            .query_row("SELECT doi FROM papers WHERE id = 'p-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(doi, "10.1038/s41586-020-2649-2");
        let row = latest_identity_check(&conn, "p-1", IdentityPhase::PreFetch)
            .unwrap()
            .expect("the correction is recorded as a check");
        assert_eq!(row.doi_corrected_from.as_deref(), Some("10.9999/wrong.1"));
        assert_eq!(row.status, IdentityStatus::Ok);
    }

    /// A corrected DOI does not re-ask the question a person already closed.
    #[test]
    fn a_doi_correction_does_not_overturn_an_override() {
        let fx = Fx::new();
        fx.paper("p-1");
        let mut conn = fx.db.conn().unwrap();
        override_identity(&mut conn, "p-1", "already settled by hand").unwrap();
        assert!(
            !correct_doi(&mut conn, "p-1", "10.9999/wrong.1", "10.1038/x").unwrap(),
            "the correction is not recorded as a check"
        );
        let doi: String = conn
            .query_row("SELECT doi FROM papers WHERE id = 'p-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(doi, "10.1038/x", "the DOI is still corrected");
        assert_eq!(
            latest_identity_check(&conn, "p-1", IdentityPhase::PreFetch)
                .unwrap()
                .unwrap()
                .status,
            IdentityStatus::Overridden
        );
    }

    /// `osti_id` is a targeted single-column write, so nothing that saves a
    /// whole `Paper` can null it — the hazard the targeted reader's docs name.
    #[test]
    fn the_osti_id_is_read_and_written_on_its_own() {
        let fx = Fx::new();
        fx.paper("p-1");
        fx.paper("p-2");
        let conn = fx.db.conn().unwrap();
        assert_eq!(read_osti_id(&conn, "p-1").unwrap(), None);
        assert!(write_osti_id(&conn, "p-1", " 1234567 ").unwrap());
        assert_eq!(
            read_osti_id(&conn, "p-1").unwrap().as_deref(),
            Some("1234567"),
            "written trimmed, so a padded value cannot miss purl/"
        );
        assert_eq!(read_osti_id(&conn, "p-2").unwrap(), None);

        // A full `save()` of the same work must not clear it.
        let (papers, _, _, _, _) = fx.db.repositories();
        let mut paper = papers.get("p-1").unwrap().unwrap();
        paper.title = "A corrected title".into();
        papers.save(&paper).unwrap();
        assert_eq!(
            read_osti_id(&fx.db.conn().unwrap(), "p-1")
                .unwrap()
                .as_deref(),
            Some("1234567"),
            "saving a Paper cannot wipe an identity column it does not carry"
        );
    }
}
