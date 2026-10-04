//! ADR-007, #253's last acceptance criterion:
//!
//! > raid's `p1`/`p3` NDJSON acquisition fields import via
//! > `acquire --from works.ndjson` (curation statuses stay raid's).
//!
//! Re-exported from [`crate::acquire`] so the queue import is part of the acquire
//! command rather than a second way into `acquisition_state`.
//!
//! # The rule that shapes this module
//!
//! **Raid is the authority on curation. scitadel's `acquisition_state` is about
//! wants and gaps.** Those are different questions, so the importer reads raid's
//! *acquisition* fields — which works are wanted, in what slots, where a human
//! should put a file, when to retry — and leaves raid's *curation* verdicts
//! alone. It never renames one to look like a scitadel status, and it never
//! infers a scitadel status from a raid one.
//!
//! # Why a preserved status cannot simply be stored
//!
//! `acquisition_state.status` is `CHECK`-constrained to ADR-007 §2's fourteen
//! spellings. A raid curation status is a different vocabulary, so there are
//! exactly three available answers and this module takes the only one that keeps
//! both facts:
//!
//! 1. **It is already one of ADR-007 §2's spellings** (case-insensitively) →
//!    written through verbatim. There is nothing to preserve: scitadel and raid
//!    are saying the same word.
//! 2. **It is not** → `status = 'pending'`, and raid's verdict is preserved
//!    **verbatim** in `acquisition_state.reason` and in
//!    [`NdjsonImport::preserved_statuses`]. `pending` is a claim about *scitadel*
//!    — "never tried by us", which is true and is the queue's own meaning in
//!    ADR-007 §2 — not a translation of raid's verdict. The verdict is never
//!    dropped, never paraphrased, and never turned into a *different* raid-looking
//!    status.
//! 3. **Refusing the row** would lose the gap entirely, which is how a work
//!    disappears from `coverage`. Not an option.
//!
//! # The ambiguities, and the readings this module takes
//!
//! No schema, fixture or issue in this workspace pins raid's `p1` / `p3` field
//! names, so each is named here with the reading taken. Every one of them is
//! **reported rather than assumed away**: [`NdjsonImport::unknown_fields`] lists
//! every key this importer does not read, and `unsupported_kinds` every artefact
//! kind it cannot file, so #247's "the raid maintainer confirms the model covers
//! raid's queue files without loss" is checkable by reading one report rather
//! than by reading this file and guessing.
//!
//! | concept | spellings accepted | reading |
//! |---|---|---|
//! | work | `doi`, `DOI`, `doi_normalized`, `doi_normalised`, `paper_id`, `id`, `pmcid`, `pmc_id`, `osti_id` | resolved against the `papers` rows this library already holds; **no paper is ever created** (see below) |
//! | wants | `artefacts`, `wanted`, `wants`, `missing` (array of strings or objects), or a singular `kind` | each element is one `acquisition_state` row |
//! | curation status | `status`, `curation_status`, `curation`, `state` | **raid's**, preserved per the rules above |
//! | per-artefact status | `status` *inside* a want object | overrides the row-level status **for that want only** |
//! | human action | `hint_url`, `url`, `landing_page` | a person opening the page |
//! | drop point | `drop_path` | where a person should put the file |
//! | retry | `next_attempt_at`, `retry_at`, `retry_after` | RFC 3339; `--resume` reads it |
//! | identity | `expected_title` / `title`, `expected_year` / `year`, `expected_first_author` / `first_author` | **reported, not applied** (see below) |
//!
//! Three readings worth stating outright:
//!
//! - **A `doi` is matched against `papers.doi`**, with a resolver prefix
//!   (`https://doi.org/`, `doi:`) stripped, because a queue file written by a
//!   spreadsheet almost certainly has one and scitadel stores a bare DOI.
//! - **No `papers` row is created.** A row naming a work this library does not
//!   have is reported under `unknown`, which is the same answer `acquire_queue_add`
//!   gives. Creating 594 paper rows from a foreign file is a decision about the
//!   corpus that belongs to the maintainer, not to an importer.
//! - **An existing gap row is left exactly as it is**, for
//!   [`crate::acquire::queue_add`]'s reason: a `needs_ill` row is a human's
//!   decision, and re-importing a queue must not return it to `pending`.
//!
//! ## The identity fields are reported, not applied
//!
//! `expected_title` / `expected_year` / `expected_first_author` are what ADR-007
//! §3's pre-fetch check compares against. The check reads **the stored `papers`
//! row**, so the only way to make a queued expectation reach it is to overwrite
//! that row — and this importer will not overwrite a work's title because a
//! foreign file said so. So an expectation that disagrees with the stored paper is
//! reported in [`NdjsonImport::identity_disagreements`] with both titles, and a
//! person decides. An expectation that *agrees* is redundant and simply not
//! reported.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use scitadel_core::models::Paper;
use scitadel_db::sqlite::{Database, FULLTEXT_LOCATOR, StateWrite, status_vocab};
use serde::Serialize;

use crate::acquire::AcquireError;

/// The `acquisition_state.status` a work with no scitadel-side history carries.
///
/// ADR-007 §2: "never tried — picked up by `acquire`". A true statement about
/// scitadel, whatever raid's curation verdict says; see the module docs.
pub const STATUS_PENDING: &str = "pending";

/// The version a want asks for when the queue does not say: the version of
/// record, matching ADR-007 §2's column default and every other writer.
pub const WANTED_VERSION: &str = "vor";

/// The key lists a top-level row is read through — one per concept, used for
/// reading *and* for the known/unknown decision, for the reason
/// [`want_fields`] documents.
const DOI_KEYS: [&str; 4] = ["doi", "DOI", "doi_normalized", "doi_normalised"];
const PAPER_ID_KEYS: [&str; 2] = ["paper_id", "id"];
const PMCID_KEYS: [&str; 2] = ["pmcid", "pmc_id"];
const OSTI_KEYS: [&str; 1] = ["osti_id"];
const WANT_LIST_KEYS: [&str; 4] = ["artefacts", "wanted", "wants", "missing"];
const ROW_STATUS_KEYS: [&str; 4] = ["curation_status", "curation", "status", "state"];
const ROW_KIND_KEYS: [&str; 1] = ["kind"];

/// Every key a top-level row may carry, derived from the lists above.
fn known_fields() -> Vec<&'static str> {
    DOI_KEYS
        .iter()
        .chain(PAPER_ID_KEYS.iter())
        .chain(PMCID_KEYS.iter())
        .chain(OSTI_KEYS.iter())
        .chain(WANT_LIST_KEYS.iter())
        .chain(ROW_KIND_KEYS.iter())
        .chain(ROW_STATUS_KEYS.iter())
        .chain(HINT_KEYS.iter())
        .chain(DROP_KEYS.iter())
        .chain(RETRY_KEYS.iter())
        // The identity expectations are read — and deliberately only *reported*,
        // never applied (see the module docs), so they are known fields.
        .chain(["expected_title", "title", "expected_year", "year"].iter())
        .chain(["expected_first_author", "first_author"].iter())
        .copied()
        .collect()
}

/// A raid curation status, verbatim, and where it landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreservedStatus {
    pub paper_id: String,
    pub kind: String,
    pub locator: String,
    /// raid's own spelling, character for character. Never normalised.
    pub raid_status: String,
    /// What `acquisition_state.status` holds afterwards — raid's own word when it
    /// was already one of ADR-007 §2's, `pending` otherwise.
    pub stored_as: String,
    /// Whether the verbatim string is also in `acquisition_state.reason`. Always
    /// `true` unless it was stored as-is, where `status` already says it.
    pub in_reason: bool,
}

/// A raid expectation that disagrees with the work scitadel holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityDisagreement {
    pub paper_id: String,
    /// raid's expectation, verbatim.
    pub expected_title: String,
    /// What the `papers` row says.
    pub stored_title: String,
}

/// What one import did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct NdjsonImport {
    pub source: PathBuf,
    /// Lines read from the file, including any that turned out not to be usable.
    pub rows_read: usize,
    /// Lines that were not a JSON object (blank lines, an array, a scalar).
    pub rows_not_objects: usize,
    /// `acquisition_state` rows created by this import.
    pub gaps_written: usize,
    /// The works that got at least one gap, in file order.
    pub works: Vec<String>,
    /// Every raid curation status this import read, verbatim. The list a person
    /// reads to confirm nothing was translated.
    pub preserved_statuses: Vec<PreservedStatus>,
    /// Wants that already existed and were **left exactly as they were**.
    pub untouched_existing_gaps: usize,
    /// Rows that named no work this library holds.
    pub unknown: Vec<String>,
    /// Every key in the file this importer does not read. Non-empty means the
    /// queue's real shape differs from the readings above.
    pub unknown_fields: BTreeSet<String>,
    /// Artefact kinds outside `acquisition_state.kind`'s vocabulary, and how
    /// often each appeared.
    pub unsupported_kinds: BTreeMap<String, usize>,
    /// raid's identity expectations that disagree with the stored work.
    pub identity_disagreements: Vec<IdentityDisagreement>,
}

/// One wanted artefact as the queue file states it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Want {
    kind: String,
    locator: String,
    /// raid's status for *this* want, when it carried one.
    status: Option<String>,
    reason: Option<String>,
    hint_url: Option<String>,
    drop_path: Option<String>,
    next_attempt_at: Option<String>,
    wanted_version: Option<String>,
}

/// Import a raid NDJSON queue into `acquisition_state`.
///
/// One `acquisition_state` row per wanted artefact, on works this library already
/// holds. Idempotent: a re-import of an unchanged file writes nothing, because
/// [`scitadel_db::sqlite::write_acquisition_states`]' `WHERE` guard skips a row
/// whose values did not change — including one whose only difference would be
/// `updated_at`.
///
/// # Errors
///
/// The file could not be read, or a line that starts a JSON value is malformed.
/// Per-row trouble is never an error: a row naming an unknown work, an
/// unsupported kind or a want that already exists lands in the report.
pub fn import_ndjson_queue(db: &Database, source: &Path) -> Result<NdjsonImport, AcquireError> {
    let body = std::fs::read_to_string(source).map_err(|e| {
        AcquireError::Adapter(crate::error::AdapterError::Other(format!(
            "could not read the NDJSON queue {}: {e}",
            source.display()
        )))
    })?;
    let mut report = NdjsonImport {
        source: source.to_path_buf(),
        ..NdjsonImport::default()
    };

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        report.rows_read += 1;
        let value: serde_json::Value = serde_json::from_str(line).map_err(|e| {
            AcquireError::Adapter(crate::error::AdapterError::Other(format!(
                "{}: line {} is not a JSON value: {e}. NDJSON is one complete JSON object per \
                 line, so a pretty-printed file has to be one object per line.",
                source.display(),
                report.rows_read
            )))
        })?;
        let Some(object) = value.as_object() else {
            report.rows_not_objects += 1;
            continue;
        };
        import_row(db, object, &mut report)?;
    }
    Ok(report)
}

/// One line of the queue file.
fn import_row(
    db: &Database,
    object: &serde_json::Map<String, serde_json::Value>,
    report: &mut NdjsonImport,
) -> Result<(), AcquireError> {
    let known = known_fields();
    for key in object.keys() {
        if !known.contains(&key.as_str()) {
            report.unknown_fields.insert(key.clone());
        }
    }

    let Some(paper) = resolve_work(db, object, report) else {
        return Ok(());
    };
    let paper_id = paper.id.as_str().to_string();

    // The row-level status, when there is one. It applies to every want in this
    // row that does not carry its own — the reading stated in the module docs.
    let row_status = first_str(object, &ROW_STATUS_KEYS);
    let hint_url = first_str(object, &HINT_KEYS);
    let drop_path = first_str(object, &DROP_KEYS);
    let next_attempt_at = first_str(object, &RETRY_KEYS);

    let wants = wants_of(object, report);
    if wants.is_empty() {
        tracing::debug!(
            paper_id,
            "queue row names a work but no wanted artefact; nothing recorded"
        );
        return Ok(());
    }

    note_identity_disagreement(&paper, object, report);

    for want in wants {
        let Some(kind) = supported_kind(&want.kind) else {
            *report
                .unsupported_kinds
                .entry(want.kind.clone())
                .or_insert(0) += 1;
            tracing::warn!(
                paper_id,
                kind = %want.kind,
                "{} is not an `acquisition_state.kind` this build knows (ADR-007 §2), so no want \
                 row was written for it",
                want.kind
            );
            continue;
        };
        let (status, in_reason) = stored_status(want.status.as_deref().or(row_status.as_deref()));
        let raid_status = want.status.clone().or_else(|| row_status.clone());

        // An existing row is a decision somebody else made — a person, or a walk
        // that reached a publisher. Left exactly as it stands, for
        // `acquire_queue_add`'s reason.
        if db
            .acquisition_state(&paper_id, &kind, &want.locator)?
            .is_some()
        {
            report.untouched_existing_gaps += 1;
            continue;
        }

        // The verbatim string goes in beside scitadel's own reason when raid
        // supplied one, so a reader of the row sees both. Never a replacement.
        let reason = match (
            want.reason.clone(),
            in_reason.then(|| raid_status.clone()).flatten(),
        ) {
            (Some(raid_reason), Some(raid_status)) => Some(format!(
                "{raid_reason} (raid curation status, preserved verbatim: {raid_status})"
            )),
            (Some(raid_reason), None) => Some(raid_reason),
            (None, Some(raid_status)) => Some(format!(
                "raid curation status, preserved verbatim: {raid_status}"
            )),
            (None, None) => None,
        };
        db.upsert_acquisition_state(&StateWrite {
            paper_id: paper_id.clone(),
            kind: kind.clone(),
            locator: want.locator.clone(),
            wanted_version: want
                .wanted_version
                .clone()
                .unwrap_or_else(|| WANTED_VERSION.to_string()),
            status: status.clone(),
            reason,
            // `None`, and never guessed: this importer resolves a work by DOI and
            // then classifies nothing. `action_list` derives the publisher from
            // the DOI registry at report time regardless (#261).
            publisher: None,
            hint_url: want.hint_url.clone().or_else(|| hint_url.clone()),
            drop_path: want.drop_path.clone().or_else(|| drop_path.clone()),
            next_attempt_at: want
                .next_attempt_at
                .clone()
                .or_else(|| next_attempt_at.clone()),
            updated_at: Utc::now().to_rfc3339(),
        })?;
        report.gaps_written += 1;
        if !report.works.contains(&paper_id) {
            report.works.push(paper_id.clone());
        }
        if let Some(raid) = raid_status {
            report.preserved_statuses.push(PreservedStatus {
                paper_id: paper_id.clone(),
                kind: kind.clone(),
                locator: want.locator.clone(),
                stored_as: status.clone(),
                in_reason,
                raid_status: raid,
            });
        }
    }
    Ok(())
}

/// Resolve the line's work against the `papers` rows this library holds.
///
/// Never creates one — see the module docs.
fn resolve_work(
    db: &Database,
    object: &serde_json::Map<String, serde_json::Value>,
    report: &mut NdjsonImport,
) -> Option<Paper> {
    if let Some(doi) = first_str(object, &DOI_KEYS)
        && let Some(paper) = paper_for_doi(db, &doi)
    {
        return Some(paper);
    }
    if let Some(id) = first_str(object, &PAPER_ID_KEYS)
        && let Ok(Some(paper)) = crate::acquire::paper_of(db, &id)
    {
        return Some(paper);
    }
    // The non-DOI identifiers ADR-007 §3 adds for works a DOI cannot name: old
    // radiochemistry and DOE reports. Two separate statements rather than one
    // query with an interpolated column name.
    if let Some(value) = first_str(object, &PMCID_KEYS)
        && let Some(paper) = paper_for_column(db, "SELECT id FROM papers WHERE pmcid = ?1", &value)
    {
        return Some(paper);
    }
    if let Some(value) = first_str(object, &OSTI_KEYS)
        && let Some(paper) =
            paper_for_column(db, "SELECT id FROM papers WHERE osti_id = ?1", &value)
    {
        return Some(paper);
    }
    let label = first_str(
        object,
        &["doi", "DOI", "paper_id", "id", "pmcid", "pmc_id", "osti_id"],
    )
    .unwrap_or_else(|| "(no identifier)".to_string());
    tracing::warn!(
        "queue row names {label}, which is not a work this library holds; no gap was recorded"
    );
    report.unknown.push(label);
    None
}

/// The `papers` row named by a `SELECT id …` lookup, for the two non-DOI
/// identifiers ADR-007 §3 adds.
///
/// The statement is passed in rather than built from a column name, so there is
/// no path by which a field name from the queue file reaches SQL as an
/// identifier.
fn paper_for_column(db: &Database, select: &str, value: &str) -> Option<Paper> {
    let conn = db.conn().ok()?;
    let id: String = conn.query_row(select, [value], |row| row.get(0)).ok()?;
    crate::acquire::paper_of(db, &id).ok().flatten()
}

/// The `papers` row for `raw_doi`, tolerating a resolver prefix.
fn paper_for_doi(db: &Database, raw_doi: &str) -> Option<Paper> {
    let normalised = scitadel_core::models::normalize_doi(raw_doi);
    let conn = db.conn().ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT id FROM papers
              WHERE doi IS NOT NULL AND REPLACE(LOWER(doi), 'https://doi.org/', '') = ?1",
        )
        .ok()?;
    let id: String = stmt
        .query_row([normalised.as_str()], |row| row.get(0))
        .ok()?;
    drop(stmt);
    crate::acquire::paper_of(db, &id).ok().flatten()
}

/// The wanted artefacts one line states.
///
/// A singular `kind` is honoured when no array field is present, so a file that
/// carries one want per line works as well as one that carries a list.
fn wants_of(
    object: &serde_json::Map<String, serde_json::Value>,
    report: &mut NdjsonImport,
) -> Vec<Want> {
    let mut wants = Vec::new();
    for key in WANT_LIST_KEYS {
        let Some(value) = object.get(key) else {
            continue;
        };
        let Some(entries) = value.as_array() else {
            // A single object rather than a list of them.
            wants.push(want_from(object, None));
            continue;
        };
        for entry in entries {
            match entry {
                serde_json::Value::String(kind) => wants.push(Want {
                    kind: kind.clone(),
                    locator: String::new(),
                    status: None,
                    reason: None,
                    hint_url: None,
                    drop_path: None,
                    next_attempt_at: None,
                    wanted_version: None,
                }),
                serde_json::Value::Object(fields) => {
                    let known = want_fields();
                    for unknown in fields.keys() {
                        if !known.contains(&unknown.as_str()) {
                            report.unknown_fields.insert(unknown.clone());
                        }
                    }
                    wants.push(want_from(fields, Some(object)));
                }
                _ => tracing::debug!("ignored a non-string, non-object artefact entry"),
            }
        }
    }
    if wants.is_empty()
        && let Some(kind) = first_str(object, &ROW_KIND_KEYS)
    {
        wants.push(Want {
            kind,
            locator: String::new(),
            status: None,
            reason: None,
            hint_url: None,
            drop_path: None,
            next_attempt_at: None,
            wanted_version: None,
        });
    }
    wants
}

/// The key lists a want object is read through.
///
/// One definition per concept, used **both** to read the object and to decide
/// whether a key is "known". Two lists would be a lie the moment a field was
/// added to one and not the other — and the symptom would be a field scitadel
/// *reads* being reported as unread, which makes `unknown_fields` useless for the
/// one job it has.
const KIND_KEYS: [&str; 3] = ["kind", "artefact", "type"];
const LOCATOR_KEYS: [&str; 1] = ["locator"];
const LABEL_KEYS: [&str; 2] = ["label", "name"];
const WANT_STATUS_KEYS: [&str; 1] = ["status"];
const REASON_KEYS: [&str; 1] = ["reason"];
const HINT_KEYS: [&str; 3] = ["hint_url", "url", "landing_page"];
const DROP_KEYS: [&str; 1] = ["drop_path"];
const RETRY_KEYS: [&str; 3] = ["next_attempt_at", "retry_at", "retry_after"];
const VERSION_KEYS: [&str; 2] = ["wanted_version", "version"];

/// Every key a want object may carry, derived from the lists above.
fn want_fields() -> Vec<&'static str> {
    KIND_KEYS
        .iter()
        .chain(LOCATOR_KEYS.iter())
        .chain(LABEL_KEYS.iter())
        .chain(WANT_STATUS_KEYS.iter())
        .chain(REASON_KEYS.iter())
        .chain(HINT_KEYS.iter())
        .chain(DROP_KEYS.iter())
        .chain(RETRY_KEYS.iter())
        .chain(VERSION_KEYS.iter())
        .copied()
        .collect()
}

/// One want, from an artefact object (or the row itself) plus the row's own
/// fallbacks.
fn want_from(
    fields: &serde_json::Map<String, serde_json::Value>,
    row: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Want {
    let kind = first_str(fields, &KIND_KEYS).unwrap_or_default();
    let label = first_str(fields, &LABEL_KEYS);
    let locator = first_str(fields, &LOCATOR_KEYS).unwrap_or_else(|| {
        // A locator is part of the UNIQUE key, so it has to be stable — derived
        // by the *one* normaliser in the workspace rather than by string surgery
        // here.
        match label {
            Some(label) => crate::import_flat::normalise_label(&label),
            None => String::new(),
        }
    });
    let full_text = supported_kind(&kind).as_deref() == Some("fulltext");
    Want {
        kind,
        locator: if full_text {
            FULLTEXT_LOCATOR.to_string()
        } else {
            locator
        },
        status: first_str(fields, &WANT_STATUS_KEYS),
        reason: first_str(fields, &REASON_KEYS),
        hint_url: first_str(fields, &HINT_KEYS)
            .or_else(|| row.and_then(|row| first_str(row, &HINT_KEYS))),
        drop_path: first_str(fields, &DROP_KEYS)
            .or_else(|| row.and_then(|row| first_str(row, &DROP_KEYS))),
        next_attempt_at: first_str(fields, &RETRY_KEYS)
            .or_else(|| row.and_then(|row| first_str(row, &RETRY_KEYS))),
        wanted_version: first_str(fields, &VERSION_KEYS),
    }
}

/// `raid_status` mapped onto what `acquisition_state.status` can hold, and
/// whether the verbatim value also had to go into `reason`.
///
/// The whole of the module's headline rule, in one function. Never a
/// translation: an unrecognised raid status yields `pending`, which is a claim
/// about scitadel, plus the verbatim string in the reason.
pub fn stored_status(raid_status: Option<&str>) -> (String, bool) {
    let Some(raid) = raid_status.map(str::trim).filter(|s| !s.is_empty()) else {
        // No curation status at all: the queue says nothing about the work, so
        // the queue's own default is `pending`.
        return (STATUS_PENDING.to_string(), false);
    };
    let candidate = raid.to_ascii_lowercase();
    if status_vocab(&candidate).is_some() {
        // Already ADR-007 §2's own word — scitadel and raid mean the same thing
        // by it, so it passes straight through and there is nothing extra to keep.
        return (candidate, false);
    }
    (STATUS_PENDING.to_string(), true)
}

/// `acquisition_state.kind` for a queue's artefact spelling, or `None`.
///
/// `fulltext` and the three `fulltext_*` spellings all collapse to the one want a
/// fetch can close (ADR-007 §3: every route is a full-text route), because a want
/// is "a full text", not "a PDF". `supported_kind("fulltext_pdf")` therefore
/// answers `fulltext` — a queue that names a PDF specifically is still asking for
/// the full text, and the ladder is what decides the serialisation.
fn supported_kind(raw: &str) -> Option<String> {
    let kind = raw.trim().to_ascii_lowercase();
    Some(match kind.as_str() {
        "fulltext" | "fulltext_pdf" | "fulltext_html" | "fulltext_xml" | "full-text" => {
            "fulltext".to_string()
        }
        "si" | "supplementary" | "supplement" | "supporting_information" => "si".to_string(),
        "table" | "tables" => "table".to_string(),
        "figure" | "figures" | "fig" => "figure".to_string(),
        _ => return None,
    })
}

/// Report a raid identity expectation that disagrees with the stored work.
///
/// Never applied: overwriting a work's title because a foreign file said so is
/// exactly the guess #260 and #261 exist to stop. Reported with both titles so a
/// person decides.
fn note_identity_disagreement(
    paper: &Paper,
    object: &serde_json::Map<String, serde_json::Value>,
    report: &mut NdjsonImport,
) {
    let expected = first_str(object, &["expected_title", "title"]);
    let Some(expected) = expected.filter(|title| !title.trim().is_empty()) else {
        return;
    };
    if expected.trim() == paper.title.trim() {
        return;
    }
    report.identity_disagreements.push(IdentityDisagreement {
        paper_id: paper.id.as_str().to_string(),
        expected_title: expected.trim().to_string(),
        stored_title: paper.title.clone(),
    });
}

/// The first present, non-blank string among `keys`.
fn first_str(object: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .next()
}

#[cfg(test)]
mod tests {
    //! The mapping rules, tested on their own so a change to one cannot be
    //! justified by a change to another.

    use super::*;
    use serde_json::json;

    fn object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("an object").clone()
    }

    /// The headline rule, in a table: what `status` holds, and whether raid's own
    /// word has to be kept somewhere beside it.
    ///
    /// The cases are chosen for the directions that matter and not for tidiness:
    /// a raid status that *is* an ADR-007 §2 word must pass through unchanged, one
    /// that is not must not be turned into a scitadel word that reads like it, and
    /// a blank or absent status is the queue saying nothing at all rather than
    /// scitadel inventing an opinion.
    #[test]
    fn stored_status_preserves_raid_and_never_translates() {
        use scitadel_db::sqlite::ALL_STATUSES;

        // Every one of ADR-007 §2's own words passes through, whatever the case
        // the queue file used. The list is the schema's, not a hand-written one,
        // so adding a status there makes this test cover it rather than let it
        // fall through to the `pending` arm and be quietly translated.
        assert_eq!(
            ALL_STATUSES.len(),
            13,
            "ADR-007 §2's table has 13 statuses; a new one must be covered here"
        );
        for vocab in ALL_STATUSES {
            let raid = vocab.status;
            assert_eq!(
                stored_status(Some(raid)),
                (raid.to_string(), false),
                "{raid} is already one of ADR-007 §2's own words"
            );
            // And case-insensitively, because a queue file written by a spreadsheet
            // is not a schema.
            let shouted = raid.to_ascii_uppercase();
            assert_eq!(
                stored_status(Some(&shouted)),
                (raid.to_string(), false),
                "{shouted} is the same word"
            );
        }

        // raid's own vocabulary: `pending`, and raid's verdict verbatim in
        // `reason`. Never a scitadel word that reads like raid's meaning.
        for raid in [
            "accepted_by_reviewer",
            "needs_reviewer",
            "rejected",
            "in_progress",
            "hold",
            "screen_out",
        ] {
            let (status, in_reason) = stored_status(Some(raid));
            assert_eq!(
                status, STATUS_PENDING,
                "{raid} must not become a scitadel word"
            );
            assert!(
                in_reason,
                "{raid} must be preserved verbatim in the reason beside it"
            );
            assert!(
                !ALL_STATUSES.iter().any(|v| v.status == raid),
                "precondition: {raid} is not one of ADR-007 §2's words, so storing it would \
                 have been a translation"
            );
        }

        // Nothing said at all is the queue saying nothing, not scitadel inventing
        // an opinion about the work.
        assert_eq!(stored_status(None), (STATUS_PENDING.to_string(), false));
        assert_eq!(
            stored_status(Some("   ")),
            (STATUS_PENDING.to_string(), false)
        );
    }

    #[test]
    fn artefact_kind_spellings_collapse_to_the_want_vocabulary() {
        // The `fulltext_*` spellings collapse: a want is "a full text", not "a
        // PDF", and the ladder chooses the serialisation.
        for spelling in [
            "fulltext",
            "fulltext_pdf",
            "fulltext_html",
            "fulltext_xml",
            "full-text",
        ] {
            assert_eq!(
                supported_kind(spelling).as_deref(),
                Some("fulltext"),
                "{spelling}"
            );
        }
        for spelling in [
            "SI",
            "si",
            "supplementary",
            "supplement",
            "supporting_information",
        ] {
            assert_eq!(
                supported_kind(spelling).as_deref(),
                Some("si"),
                "{spelling}"
            );
        }
        assert_eq!(supported_kind("tables").as_deref(), Some("table"));
        assert_eq!(supported_kind("figures").as_deref(), Some("figure"));
        // Outside the vocabulary: no row, and the report names it.
        for spelling in ["dataset", "protocol", "video", "", "full_text"] {
            assert_eq!(supported_kind(spelling), None, "{spelling:?}");
        }
    }

    /// A want's locator is part of the UNIQUE key, so it is derived by the one
    /// normaliser in the workspace rather than by string surgery here — the same
    /// rule `import_flat` and `scan` use, so one file cannot get two ids.
    #[test]
    fn a_wants_locator_comes_from_the_shared_normaliser() {
        let row = object(json!({
            "artefacts": [{"kind": "si", "label": "Supporting Information S1"}]
        }));
        let mut report = NdjsonImport::default();
        let wants = wants_of(&row, &mut report);
        assert_eq!(wants[0].locator, "supporting-information-s1");
        assert_eq!(
            wants[0].locator,
            crate::import_flat::normalise_label("Supporting Information S1")
        );

        // An explicit locator wins, and the full-text slot forces the empty one.
        let explicit = object(json!({
            "artefacts": [{"kind": "si", "label": "S1", "locator": "custom-key"}]
        }));
        assert_eq!(wants_of(&explicit, &mut report)[0].locator, "custom-key");
        let full_text = object(json!({
            "artefacts": [{"kind": "fulltext_pdf", "label": "ignored"}]
        }));
        assert_eq!(
            wants_of(&full_text, &mut report)[0].locator,
            FULLTEXT_LOCATOR,
            "a full-text want is `(paper_id, 'fulltext', '')`; a locator would make it a second \\
             row"
        );
    }

    /// A singular `kind` is honoured when no list field is present, so a file with
    /// one want per line works as well as one that carries a list — and a row
    /// carrying both uses the list.
    #[test]
    fn a_singular_kind_is_honoured_and_a_list_wins_over_it() {
        let mut report = NdjsonImport::default();
        let singular = object(json!({"kind": "fulltext"}));
        let wants = wants_of(&singular, &mut report);
        assert_eq!(wants.len(), 1);
        assert_eq!(wants[0].kind, "fulltext");

        let both = object(json!({"kind": "fulltext", "artefacts": ["si", "table"]}));
        let wants = wants_of(&both, &mut report);
        assert_eq!(wants.len(), 2, "the list is the specific claim: {wants:?}");
        assert_eq!(
            wants.iter().map(|w| w.kind.as_str()).collect::<Vec<_>>(),
            vec!["si", "table"]
        );

        // A row naming nothing wanted records nothing.
        assert!(wants_of(&object(json!({"doi": "10.0/x"})), &mut report).is_empty());
    }

    /// A per-artefact status overrides the row-level one for that want only, which
    /// is the reading stated in the module docs.
    #[test]
    fn a_per_artefact_status_overrides_the_row_level_one() {
        let row = object(json!({
            "status": "accepted_by_reviewer",
            "artefacts": [
                {"kind": "fulltext"},
                {"kind": "si", "label": "S1", "status": "needs_ill"}
            ]
        }));
        let mut report = NdjsonImport::default();
        let wants = wants_of(&row, &mut report);
        assert_eq!(
            wants[0].status, None,
            "the si's own status must not leak upwards"
        );
        assert_eq!(wants[1].status.as_deref(), Some("needs_ill"));
    }

    /// `unknown_fields` is the list #247's "without loss" checklist is checked
    /// against, so a key this build *reads* must never appear on it — that was a
    /// real bug while the read-lists and the known-list were separate.
    #[test]
    fn a_field_this_build_reads_is_never_reported_as_unread() {
        let mut report = NdjsonImport::default();
        wants_of(
            &object(json!({
                "artefacts": [{
                    "kind": "si", "label": "S1", "locator": "s1", "status": "needs_ill",
                    "reason": "no route", "hint_url": "https://example.org",
                    "drop_path": "/tmp/S1.pdf", "next_attempt_at": "2026-01-01T00:00:00+00:00",
                    "wanted_version": "vor"
                }]
            })),
            &mut report,
        );
        assert!(
            report.unknown_fields.is_empty(),
            "{:?}",
            report.unknown_fields
        );
        // And the row-level fallbacks are read through the same lists.
        let row = object(json!({
            "doi": "10.0/x", "hint_url": "https://example.org", "drop_path": "/tmp",
            "next_attempt_at": "2026-01-01T00:00:00+00:00", "expected_title": "T",
            "artefacts": ["fulltext"]
        }));
        for key in row.keys() {
            assert!(
                known_fields().contains(&key.as_str()),
                "{key} is read, so it must not be reported as unknown"
            );
        }
        // A key that is genuinely not read is reported rather than dropped. The
        // reporting happens in `import_row`, which owns the top-level object; here
        // the claim is only that the key is *not* in the known list, which is what
        // `import_row` tests against.
        assert!(
            !known_fields().contains(&"curation_note"),
            "curation_note is raid's own prose and this build does not read it, so it must be \
             reported rather than silently ignored"
        );
    }
}
