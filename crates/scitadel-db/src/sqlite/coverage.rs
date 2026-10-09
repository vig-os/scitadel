//! ADR-007 §1 "Have" (derived) and §2 "Status vocabulary": the readers
//! behind `scitadel coverage` and `scitadel action_list`.
//!
//! # There is no "have" status
//!
//! ADR-007 §1, "Have" (derived):
//!
//! > For a wanted kind of `fulltext`, the work *has* it when any
//! > `fulltext_*` artefact exists with `access_status = 'full_text'`, and its
//! > `version` satisfies `wanted_version`.
//! >
//! > - `'any'` accepts every version.
//! > - `'vor'` accepts `vor`. It also accepts `unknown`, but **only** on rows
//! >   from `route IN ('legacy','import_flat')`, since those predate version
//! >   tracking.
//! > - A held version that doesn't satisfy the wanted one yields
//! >   `wrong_version`.
//! >
//! > For specific kinds (`si`, `table`, `figure`), the rule is the same with
//! > the exact kind and locator.
//!
//! [`version_satisfies`] is that rule and nothing else; [`is_satisfied`] adds
//! the two clauses around it (which artefact kinds a want accepts, and which
//! locators). Nothing in this module reads a stored "have" answer, because
//! there is none to read: `acquisition_state` records only what we want and
//! have not got, so a row there is evidence of a *gap*, never of a gap's
//! closure. Deriving it in one place is the whole reason this module exists —
//! a second derivation in the CLI would be a second thing to get wrong.
//!
//! # A work with no want is not missing anything
//!
//! The report counts a work as missing only when a want row survives the
//! derivation above. A work with neither a want row nor an artefact has no
//! recorded gap, so it is reported as *untracked* ([`CoverageReport`]) and
//! counted in neither column. Reporting it as missing would be inventing a
//! requirement nobody stated — the mirror image of the three defects #261,
//! #260 and #275 were filed for.
//!
//! # `action_list` is a projection of the same computation
//!
//! ADR-007 §2: "`action_list` groups rows by **(action group, publisher)** and
//! carries `drop_path` and `hint_url`. Its counts sum exactly to the missing
//! totals in `coverage`."
//!
//! [`ActionList::from_report`] partitions `CoverageReport::entries` into
//! groups and deferred buckets, and it is the *only* place that classification
//! happens. It consumes the same `Vec` the coverage projection renders, so a
//! row cannot be in one view and not the other: [`ActionList::accounted`] is a
//! sum over a partition of that list rather than a second count of it, and the
//! constructor debug-asserts the two against each other.
//!
//! # The publisher is derived, never guessed
//!
//! A group's publisher key comes from the DOI registrant-prefix registry
//! ([`scitadel_core::publisher`]), not from the stored column and never from a
//! hand-maintained list — #261's whole point. When the registry cannot
//! classify the prefix, **no publisher name is printed** and the group carries
//! [`RouteVerdict::PublisherUnknown`]'s note, which says a route was never
//! evaluated. [`RouteVerdict::NoTdmRouteAvailable`] is reachable only through a
//! classified publisher, so "no TDM route available" cannot be printed for a
//! publisher nobody looked at.
//!
//! # Readers take the same answer, not a second one
//!
//! The report is not the only thing that has to know what we hold.
//! `read_paper` and `find_cached_file` have to know *which file* we hold, and
//! the TUI's download-state column has to know whether we hold it at all — and
//! all three have to agree, or a reader opens a file the report says is not
//! there.
//!
//! So they come here too: [`is_held_fulltext`] is the gate the bulk read applies,
//! [`held_fulltext_artefacts`] is that gate over the rows themselves, and
//! [`download_states`] projects it onto one column.
//! `the_report_and_a_reader_agree_on_which_artefact_is_the_full_text` holds all
//! three against a matrix of rows that disagree on every clause, because a
//! second derivation is exactly the kind of thing that passes one test and then
//! diverges on the row nobody thought of.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use rusqlite::Connection;
use scitadel_core::publisher::{PublisherVerdict, RouteVerdict, classify_publisher};
use serde::Serialize;

use crate::error::DbError;
use crate::sqlite::Database;

/// Every `acquisition_state.status` migration 013 allows, with ADR-007 §2's
/// meaning and its `action_list` group.
///
/// This is the report's own copy of the vocabulary, and
/// `the_status_vocabulary_is_exactly_the_schema_check` pins it to the live
/// schema in both directions: a status here the schema refuses, and a status
/// the schema allows that this table does not mention, are both failures. The
/// second is the dangerous one — a status nobody here knew about would be
/// dropped from `action_list`, which is precisely how the two commands would
/// stop agreeing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusVocab {
    pub status: &'static str,
    /// ADR-007 §2's "meaning" column, verbatim.
    pub meaning: &'static str,
    /// ADR-007 §2's `action_list` group, or `None` where the ADR prints "—"
    /// (no human action: ADR-007 §2 puts these in the `acquire` queue).
    pub action: Option<&'static str>,
    /// Where a row with this status goes instead, for `action_list` to print.
    /// `action_list` accounts for *every* missing entry, so it has to say what
    /// became of the ones it leaves out.
    pub goes_to: &'static str,
}

/// ADR-007 §2 "Status vocabulary", all thirteen rows.
pub const ALL_STATUSES: [StatusVocab; 13] = [
    StatusVocab {
        status: "pending",
        meaning: "never tried",
        action: None,
        goes_to: "the `acquire` queue (never tried)",
    },
    StatusVocab {
        status: "oa_fetchable",
        meaning: "an OA location is known, not yet fetched",
        action: None,
        goes_to: "the `acquire` queue (an OA location is known)",
    },
    StatusVocab {
        status: "tdm_available",
        meaning: "a sanctioned TDM route exists and credentials are present",
        action: None,
        goes_to: "the `acquire` queue (a TDM route and its credential are present)",
    },
    StatusVocab {
        status: "tdm_key_missing",
        meaning: "a sanctioned TDM route exists, no credential",
        action: Some("register a key"),
        goes_to: "the human list",
    },
    StatusVocab {
        status: "needs_login",
        meaning: "tier 4 is possible, no live session",
        action: Some("log in (scitadel browser)"),
        goes_to: "the human list",
    },
    StatusVocab {
        status: "not_entitled",
        meaning: "authenticated, still 403",
        action: Some("ILL / library request"),
        goes_to: "the human list",
    },
    StatusVocab {
        status: "needs_authorisation",
        meaning: "only bulk tier 4 could fetch it and no `tdm_authorisation` exists",
        action: Some("open & save one-click, or ask the library"),
        goes_to: "the human list",
    },
    StatusVocab {
        status: "needs_ill",
        meaning: "no route (includes works without a DOI)",
        action: Some("ILL request with an exported citation"),
        goes_to: "the human list",
    },
    StatusVocab {
        status: "unavailable",
        meaning: "the work has no such artefact (e.g. no SI)",
        action: None,
        goes_to: "no action: the work has no such artefact",
    },
    StatusVocab {
        status: "identity_mismatch",
        meaning: "the resolved or served title doesn't match the expected one",
        action: Some("check DOI (both titles shown)"),
        goes_to: "the human list",
    },
    StatusVocab {
        status: "wrong_version",
        meaning: "only a non-wanted version is held",
        action: Some("fetch the wanted version"),
        goes_to: "the human list",
    },
    StatusVocab {
        status: "rate_limited",
        meaning: "the bucket is in backoff or at its cap",
        action: None,
        goes_to: "retried at `next_attempt_at` by `acquire`",
    },
    StatusVocab {
        status: "error",
        meaning: "transient failure",
        action: None,
        goes_to: "retried with backoff by `acquire`",
    },
];

/// The `acquisition_state.kind` vocabulary (migration 013's CHECK).
///
/// Seeded into [`CoverageReport::by_kind`] with zeroes so a reader can tell
/// "no works want this kind" from "this kind is not tracked", the same reason
/// [`ALL_STATUSES`] is seeded into [`CoverageReport::by_status`].
pub const ALL_WANT_KINDS: [&str; 7] = [
    "fulltext",
    "fulltext_pdf",
    "fulltext_html",
    "fulltext_xml",
    "si",
    "table",
    "figure",
];

/// The artefact kinds a `fulltext` want accepts, in any serialisation.
pub const FULLTEXT_ARTEFACT_KINDS: [&str; 3] = ["fulltext_pdf", "fulltext_html", "fulltext_xml"];

/// The routes whose `unknown` version ADR-007 §1 trusts for a `vor` want: the
/// two that carry files predating version tracking.
pub const UNTRACKED_VERSION_ROUTES: [&str; 2] = ["legacy", "import_flat"];

/// ADR-007 §1's version rule, and nothing else.
///
/// Kept free of SQL and of `artefacts` so the fiddly part is testable on its
/// own: `any` accepts every version, `vor` accepts `vor` plus `unknown` from
/// [`UNTRACKED_VERSION_ROUTES`], and `am` / `preprint` accept exactly
/// themselves. Anything else — including a `wanted_version` no migration ever
/// wrote — is unsatisfied, so unrecognised input can never read as `any`.
///
/// Two deliberate narrownesses:
///
/// - A held VoR does **not** satisfy an `am` want. The version in hand is not
///   the version that was asked for, and ADR-007 §2 has `wrong_version` for
///   exactly that shape; the ADR's version list says nothing about a want being
///   satisfied "at least this good".
/// - `unknown` is trusted only on the two routes the ADR names. A
///   `route = 'manual'` file predates version tracking just as much, but
///   widening the list on that argument would let a hand-placed file of unknown
///   vintage report as a version of record — the overclaiming direction.
#[must_use]
pub fn version_satisfies(wanted_version: &str, held_version: &str, route: &str) -> bool {
    match wanted_version {
        "any" => true,
        "vor" => {
            held_version == "vor"
                || (held_version == "unknown" && UNTRACKED_VERSION_ROUTES.contains(&route))
        }
        "am" => held_version == "am",
        "preprint" => held_version == "preprint",
        _ => false,
    }
}

/// ADR-007 §1's gate on one stored artefact row: is this the full text we hold?
///
/// The reader-facing twin of [`is_satisfied`], and deliberately written against
/// the *row* rather than the reduced [`HeldArtefact`] so a consumer that needs the
/// artefact — to open its blob, to name its kind — can ask the same question the
/// report asks and get the same rows back.
///
/// The three clauses and where each comes from:
///
/// - `access_status = 'full_text'` is ADR-007 §1's gate verbatim. An abstract or a
///   paywall stub is bytes we hold and is not the paper.
/// - `missing_on_disk = 0` is the same addition [`read_held_artefacts`] makes in
///   SQL: the column means "the path is recorded but we hold no usable bytes", and
///   counting such a row would let a deleted file read as a held full text.
/// - `version_satisfies` is §1's version rule, unchanged, including the
///   `route IN ('legacy','import_flat')` trust for an `unknown` version.
///
/// `wanted_version` is the reader's *own* question, not a default: a reader asking
/// "do we have a file to open" passes [`ANY_VERSION`], while the report passes the
/// want's own version and can therefore answer `wrong_version`. Same gate,
/// different question — which is why it is a parameter and not a constant.
#[must_use]
pub fn is_held_fulltext(row: &crate::sqlite::artefacts::ArtefactRow, wanted_version: &str) -> bool {
    FULLTEXT_ARTEFACT_KINDS.contains(&row.kind.as_str())
        && row.access_status == scitadel_core::models::AccessStatus::FullText.label()
        && !row.missing_on_disk
        && version_satisfies(wanted_version, &row.version, &row.route)
}

/// The `wanted_version` for a reader that wants a file rather than a verdict:
/// every version is acceptable.
pub const ANY_VERSION: &str = "any";

/// Every artefact ADR-007 §1 says this work holds as its full text, best first.
///
/// The single answer `find_cached_file`, `read_paper` and the TUI's state column
/// take, so none of them decides on its own which artefact counts. A work with
/// nothing held yields an empty vector, never a guess.
///
/// ## The order is the reader's order
///
/// ADR-007 §3 ranks versions — VoR over AM over preprint — because a version of
/// record *is* the work, and that is the order the ladder fetches in. Within a
/// version the serialisation decides, and there the order is the one this product
/// has always opened in: a PDF before HTML before XML, because a PDF is what both
/// the text extractor and the OS viewer accept, and an XML serialisation exists
/// here because JATS is a real full text and nothing else was available.
///
/// Ordering is therefore total and deterministic, so two runs — and two callers —
/// pick the same file for a work that holds more than one.
pub fn held_fulltext_artefacts(
    conn: &Connection,
    paper_id: &str,
    wanted_version: &str,
) -> Result<Vec<crate::sqlite::artefacts::ArtefactRow>, DbError> {
    let mut held: Vec<_> = crate::sqlite::artefacts::read_artefacts_for_paper(conn, paper_id)?
        .into_iter()
        .filter(|row| is_held_fulltext(row, wanted_version))
        .collect();
    held.sort_by_key(|row| (version_rank(&row.version), serialisation_rank(&row.kind)));
    Ok(held)
}

/// Where a version sits in ADR-007 §3's ranking. `unknown` last: it is the
/// fallback for a file nobody could place, not a claim about the work.
fn version_rank(version: &str) -> u8 {
    match version {
        "vor" => 0,
        "am" => 1,
        "preprint" => 2,
        _ => 3,
    }
}

/// Where a serialisation sits in a reader's preference order.
fn serialisation_rank(kind: &str) -> u8 {
    match kind {
        "fulltext_pdf" => 0,
        "fulltext_html" => 1,
        _ => 2,
    }
}

/// What a reader shows in a work's download-state column.
///
/// Derived, like everything else here: there is no stored status to read, which is
/// why this type exists instead of the retired `DownloadStatus` column
/// (`scitadel_core::models::Paper` documents that one). The variants are the shapes
/// a human can act on, not the shapes the fetch chain can produce — a paywall stub
/// and an abstract are both "we hold something that is not the paper", and
/// `artefacts.access_status` is where a consumer that must tell them apart looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// We hold the full text, on disk, at a version a `fulltext` want accepts.
    FullText,
    /// We hold a full-text artefact that is **not** the full text: a paywall stub,
    /// an abstract page, or bytes nobody classified. The file exists, so this is the
    /// shape where opening it shows a human something that is not the paper.
    NotFullText,
    /// A row says we should have it and the bytes are gone (`missing_on_disk`).
    /// Distinct from holding nothing because it is a *loss*, and `scitadel scan` is
    /// what reconciles it. A work that never had a file is not this.
    Missing,
    /// Nothing held, and a recorded gap that concluded something — every status but
    /// `pending`, which is the only one that means "never tried" (ADR-007 §2) and
    /// the only one `acquire` picks up on its own.
    Gap,
    /// Nothing held and nothing recorded. No claim either way, which is
    /// deliberately not [`Self::Gap`]: reporting "not downloaded" for a work nobody
    /// asked about is inventing a requirement.
    Untracked,
}

impl DownloadState {
    /// Every value, so a caller mapping this onto a glyph is exhaustive over one
    /// list rather than a hand-written copy that drifts.
    pub const ALL: [Self; 5] = [
        Self::FullText,
        Self::NotFullText,
        Self::Missing,
        Self::Gap,
        Self::Untracked,
    ];
}

/// The derived download state of every work in `paper_ids`.
///
/// One work in, one answer out, keyed by `paper_id`. A work with nothing recorded
/// is **absent from the map** rather than mapped to `Untracked`, so a caller
/// rendering a table keeps its own "no row" default and this function does not have
/// to guess what that default is.
///
/// Two queries rather than one per work: the TUI's table asks for a thousand rows
/// on every redraw, and a per-work derivation there would be a thousand queries per
/// frame. Works not named in `paper_ids` are computed and dropped, so a caller that
/// passes a short list pays for that list only in the filter — the aggregation is
/// library-wide by design, because the alternative is a `WHERE paper_id IN (…)`
/// built by string interpolation, which is how a 1000-element SQL statement and a
/// `SQLITE_MAX_VARIABLE_NUMBER` error arrive together.
///
/// # Errors
///
/// `DbError` if SQLite fails. Nothing is refused for content.
pub fn download_states(
    conn: &Connection,
    paper_ids: &[String],
) -> Result<HashMap<String, DownloadState>, DbError> {
    let mut full: HashSet<String> = HashSet::new();
    let mut not_full: HashSet<String> = HashSet::new();
    let mut missing: HashSet<String> = HashSet::new();

    let kinds = FULLTEXT_ARTEFACT_KINDS
        .iter()
        .map(|kind| format!("'{kind}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT paper_id, access_status, missing_on_disk FROM artefacts WHERE kind IN ({kinds})"
    ))?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (paper_id, access_status, missing_on_disk) = row?;
        if missing_on_disk != 0 {
            missing.insert(paper_id);
        } else if access_status == scitadel_core::models::AccessStatus::FullText.label() {
            full.insert(paper_id);
        } else {
            not_full.insert(paper_id);
        }
    }

    let mut gaps: HashSet<String> = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT paper_id FROM acquisition_state
          WHERE kind = 'fulltext' AND locator = '' AND status <> 'pending'",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        gaps.insert(row?);
    }

    let mut states = HashMap::new();
    for paper_id in paper_ids {
        // Holding the full text wins over a gap row. `record_download` retracts the
        // want in the same transaction that writes the artefact, so this arm only
        // fires for a gap recorded *after* the fetch — and the file is on disk, so
        // it is the answer a reader needs.
        let state = if full.contains(paper_id) {
            DownloadState::FullText
        } else if not_full.contains(paper_id) {
            DownloadState::NotFullText
        } else if missing.contains(paper_id) {
            DownloadState::Missing
        } else if gaps.contains(paper_id) {
            DownloadState::Gap
        } else {
            continue;
        };
        states.insert(paper_id.clone(), state);
    }
    Ok(states)
}

/// Which artefact kinds a want of `kind` accepts.
fn kind_matches(want_kind: &str, artefact_kind: &str) -> bool {
    if want_kind == "fulltext" {
        FULLTEXT_ARTEFACT_KINDS.contains(&artefact_kind)
    } else {
        want_kind == artefact_kind
    }
}

/// Which locators a want of `kind` accepts.
///
/// The bare `fulltext` want accepts any locator, because ADR-007 §1 says "any
/// `fulltext_*` artefact exists" without qualification. Every other kind matches
/// the exact locator — the ADR's last line of §1 "Have" — so an `si` want for
/// `table-1` is not satisfied by `table-2`.
///
/// A want that names a *serialisation* (`fulltext_pdf`, `fulltext_xml`) is not
/// covered by the ADR, which only contrasts the bare `fulltext` want with
/// "`si`, `table`, `figure`". It is treated as a specific kind — exact `kind`,
/// exact `locator` — because a want for a PDF that an HTML page satisfies would
/// let the report claim a serialisation we do not hold.
fn locator_matches(want_kind: &str, want_locator: &str, artefact_locator: &str) -> bool {
    want_kind == "fulltext" || want_locator == artefact_locator
}

/// One `artefacts` row that could close a want, reduced to the four columns
/// the derivation reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HeldArtefact {
    kind: String,
    locator: String,
    version: String,
    route: String,
}

/// What ADR-007 §1 says about one want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Satisfaction {
    /// The derivation says we hold it: the want is closed.
    Held,
    /// A `full_text` artefact of the right kind and locator exists, at a
    /// version the want does not accept — ADR-007 §2's `wrong_version` shape,
    /// and a materially different problem from holding nothing at all.
    HeldAtAnotherVersion,
    /// Nothing held that could close it.
    NothingHeld,
}

/// The ADR-007 §1 derivation for one want against every held artefact of that
/// work.
fn is_satisfied(
    want_kind: &str,
    want_locator: &str,
    wanted_version: &str,
    candidates: &[HeldArtefact],
) -> Satisfaction {
    let mut any_matching_slot = false;
    for candidate in candidates {
        if !kind_matches(want_kind, &candidate.kind)
            || !locator_matches(want_kind, want_locator, &candidate.locator)
        {
            continue;
        }
        any_matching_slot = true;
        if version_satisfies(wanted_version, &candidate.version, &candidate.route) {
            return Satisfaction::Held;
        }
    }
    if any_matching_slot {
        Satisfaction::HeldAtAnotherVersion
    } else {
        Satisfaction::NothingHeld
    }
}

/// What one work still wants, derived — never asserted by a stored status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingEntry {
    pub paper_id: String,
    pub kind: String,
    pub locator: String,
    pub wanted_version: String,
    pub status: String,
    pub reason: Option<String>,
    /// The publisher **as recorded** in the row, carried for machine consumers.
    ///
    /// The human `action_list` never groups by it — it groups by the DOI
    /// registry's answer ([`PublisherKey`]), so a group heading can never
    /// contradict the route verdict printed beside it. It is `None` in practice
    /// today: `StateWrite` has no `publisher` field, so nothing in the
    /// workspace can fill the column this reads.
    pub publisher: Option<String>,
    pub hint_url: Option<String>,
    pub drop_path: Option<String>,
}

/// One want's identity inside a report: `(paper_id, kind, locator)`, the
/// `acquisition_state` primary key. A [`MissingEntry`] carries all three plus
/// the wanted version, so nothing is ever matched on a title.
type WantKey = (String, String, String);

impl MissingEntry {
    /// The want's identity, for a human line: `fulltext (vor)`,
    /// `si:table-1 (vor)`, `figure:fig-2 (any)`.
    #[must_use]
    pub fn describe(&self) -> String {
        let slot = if self.locator.is_empty() {
            self.kind.clone()
        } else {
            format!("{}:{}", self.kind, self.locator)
        };
        // The wanted version is on every line, not just the locator-less ones:
        // which version we are after is the whole difference between a held
        // preprint and a satisfied want.
        format!("{slot} ({})", self.wanted_version)
    }

    fn key(&self) -> WantKey {
        (
            self.paper_id.clone(),
            self.kind.clone(),
            self.locator.clone(),
        )
    }
}

/// Per-kind totals over *wants*, with every kind in [`ALL_WANT_KINDS`]
/// present even at zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct KindTotal {
    /// Recorded want rows of this kind.
    pub wanted: usize,
    /// Of those, the ones the derivation says we do not hold.
    pub missing: usize,
    /// `wanted - missing`: the wants the derivation says are satisfied.
    ///
    /// A count of *wants closed*, not of artefact rows — a work holding both a
    /// VoR and a preprint still contributes one `held`, and two artefacts
    /// nobody asked for contribute nothing.
    pub held: usize,
}

/// What `coverage` reports. Everything here is derived on read; nothing is
/// cached and nothing is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageReport {
    /// Recorded wants the derivation says we do not hold, ordered by
    /// `(kind, locator, paper_id)` so two runs print identically.
    pub entries: Vec<MissingEntry>,
    /// Per-kind totals, zero-filled across [`ALL_WANT_KINDS`].
    pub by_kind: BTreeMap<String, KindTotal>,
    /// Missing entries per status, zero-filled across [`ALL_STATUSES`]. A
    /// status whose wants are all satisfied shows zero — this is not the count
    /// of rows ever written with that status.
    pub by_status: BTreeMap<String, usize>,
    /// How many missing entries are cases where a `full_text` artefact of the
    /// right kind and locator *is* held, at a version the want rejects
    /// (ADR-007 §2's `wrong_version` shape). Kept beside the totals because
    /// "missing 212" reads very differently next to "31 of them are works we
    /// hold an earlier version of", and an ILL request sent for a work whose
    /// preprint is already on disk is a wasted afternoon.
    pub held_at_another_version: usize,
    /// Works in the library. The four scope counters below are library-wide and
    /// are **not** narrowed by a `--kind` filter — there is no honest per-kind
    /// version of "works in the library", and [`Self::filtered`] says so rather
    /// than passing a filtered number off as a whole one.
    pub works_total: usize,
    /// Works with at least one recorded want: the acquisition scope.
    pub works_in_scope: usize,
    /// Works with neither a want row nor an artefact. No gap has been recorded
    /// for them, so they are **not** counted as missing.
    pub untracked_works: usize,
    /// Works that hold artefacts but have no want row at all: files we have,
    /// wants nobody stated. Counted as neither covered nor missing.
    pub held_untracked_works: usize,
    /// Each work's DOI, keyed by `paper_id`, for the works this report has an
    /// entry for. Plumbing for [`ActionList::from_report`], which needs the DOI
    /// to classify a publisher (#261). It lives here because the DOI belongs to
    /// the *work*, and one work appears in many entries.
    #[serde(skip)]
    work_dois: BTreeMap<String, String>,
    /// Which missing entries are version mismatches rather than absences, so
    /// [`Self::filtered`] can recount instead of guessing.
    #[serde(skip)]
    version_mismatches: BTreeSet<WantKey>,
    /// Works whose identity a **person** has settled
    /// (`paper_identity_checks.status = 'overridden'`), with the reason they
    /// gave (#253's escape hatch).
    ///
    /// Library-wide, like [`Self::works_total`], and **not** part of the missing
    /// totals: an override is not a want and not an artefact, so counting it in
    /// either would break the accounting ADR-007 §2 requires. It is here so that
    /// both projections can *show* it, which is the difference between an
    /// override and a silent behaviour change — a work that stopped blocking
    /// because a person said so looks exactly like a work that stopped blocking
    /// because the matcher changed, and only the reason distinguishes them.
    pub identity_overrides: Vec<crate::sqlite::identity::IdentityOverride>,
}

/// Why a coverage read failed. A distinct type rather than a [`DbError`]
/// variant because the second case is bad *input*, not a broken database, and
/// its message names the valid kinds instead of naming a column.
#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    UnknownKind(#[from] UnknownWantKind),
}

/// A `kind` argument outside [`ALL_WANT_KINDS`].
///
/// Carries the offending string and the vocabulary, because the caller is about
/// to tell a person which kinds exist and a bare "invalid input" would make them
/// guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownWantKind(pub String);

impl fmt::Display for UnknownWantKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is not a tracked artefact kind; valid kinds are {}",
            self.0,
            ALL_WANT_KINDS.join(", ")
        )
    }
}

impl std::error::Error for UnknownWantKind {}

impl CoverageReport {
    /// This report restricted to one want `kind`, or `self` unchanged.
    ///
    /// `entries`, `by_kind`, `by_status` and `held_at_another_version` are
    /// recomputed over the subset, so every number in a filtered output
    /// describes the same slice. The library-wide scope counters are left alone
    /// and are documented as library-wide.
    ///
    /// # Errors
    ///
    /// [`UnknownWantKind`] if `kind` is not in [`ALL_WANT_KINDS`]. A silent
    /// no-op filter would print an empty report that reads like "nothing is
    /// missing".
    pub fn filtered(self, kind: Option<&str>) -> Result<Self, UnknownWantKind> {
        let Some(kind) = kind else {
            return Ok(self);
        };
        if !ALL_WANT_KINDS.contains(&kind) {
            return Err(UnknownWantKind(kind.to_string()));
        }
        let entries: Vec<MissingEntry> = self
            .entries
            .into_iter()
            .filter(|e| e.kind == kind)
            .collect();
        let mut by_kind = BTreeMap::new();
        by_kind.insert(
            kind.to_string(),
            KindTotal {
                wanted: entries.len(),
                missing: entries.len(),
                held: 0,
            },
        );
        let mut by_status = empty_status_totals();
        for entry in &entries {
            *by_status.entry(entry.status.clone()).or_insert(0) += 1;
        }
        let kept: BTreeSet<WantKey> = entries.iter().map(MissingEntry::key).collect();
        let version_mismatches: BTreeSet<WantKey> = self
            .version_mismatches
            .intersection(&kept)
            .cloned()
            .collect();
        let work_dois = self
            .work_dois
            .into_iter()
            .filter(|(paper_id, _)| kept.iter().any(|(p, _, _)| p == paper_id))
            .collect();
        Ok(CoverageReport {
            entries,
            by_kind,
            by_status,
            held_at_another_version: version_mismatches.len(),
            version_mismatches,
            work_dois,
            works_total: self.works_total,
            works_in_scope: self.works_in_scope,
            untracked_works: self.untracked_works,
            held_untracked_works: self.held_untracked_works,
            identity_overrides: self.identity_overrides,
        })
    }

    /// Total recorded wants, across every kind.
    #[must_use]
    pub fn wanted_total(&self) -> usize {
        self.by_kind.values().map(|k| k.wanted).sum()
    }

    /// The number `action_list` must account for: one per recorded want the
    /// derivation says we do not hold.
    #[must_use]
    pub fn missing_total(&self) -> usize {
        self.entries.len()
    }

    /// Recorded wants the derivation says are satisfied.
    #[must_use]
    pub fn held_total(&self) -> usize {
        self.wanted_total().saturating_sub(self.missing_total())
    }

    /// The DOI of `paper_id`, when this report knows one.
    #[must_use]
    pub fn doi_of(&self, paper_id: &str) -> Option<&str> {
        self.work_dois.get(paper_id).map(String::as_str)
    }

    /// The `action_list` view: the same entries, grouped for a human.
    ///
    /// The single place that classification happens. See [`ActionList`].
    #[must_use]
    pub fn action_list(&self) -> ActionList {
        ActionList::from_report(self.clone())
    }
}

/// `by_status` seeded with every ADR-007 §2 status at zero.
fn empty_status_totals() -> BTreeMap<String, usize> {
    ALL_STATUSES
        .iter()
        .map(|v| (v.status.to_string(), 0))
        .collect()
}

/// `by_kind` seeded with every migration 013 want kind at zero.
fn empty_kind_totals() -> BTreeMap<String, KindTotal> {
    ALL_WANT_KINDS
        .iter()
        .map(|k| ((*k).to_string(), KindTotal::default()))
        .collect()
}

/// The ADR-007 §2 row for `status`, if this build's vocabulary knows it.
#[must_use]
pub fn status_vocab(status: &str) -> Option<&'static StatusVocab> {
    ALL_STATUSES.iter().find(|v| v.status == status)
}

/// How a group's works are identified when there is no publisher name to print.
///
/// The whole #261 problem in one type. [`Self::label`] returns `Some` for
/// exactly one variant, so there is a single place in this module a publisher
/// name can come from and it is the DOI registry — never the stored
/// `acquisition_state.publisher` column and never a string built at a call
/// site. The two unclassified variants keep *why* there is no name, and they
/// stay separate keys, so a group about prefix `99999` never borrows the
/// wording of a group about `88888`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum PublisherKey {
    /// The registry classifies the prefix. [`Self::label`] is its answer.
    Classified(String),
    /// The prefix is not in the registry, so no route was evaluated and no
    /// publisher may be named. The prefix is carried so the report can name
    /// *what* is missing from the table instead of only saying that something
    /// is.
    Unclassified(String),
    /// No DOI on record: nothing to classify, and nothing was evaluated.
    /// ADR-007 §2's `needs_ill` names this case explicitly ("includes works
    /// without a DOI").
    NoDoi,
}

impl PublisherKey {
    /// Classify a work through the DOI registry (#261).
    ///
    /// The stored `acquisition_state.publisher` column is deliberately not an
    /// input: `StateWrite` has no field for it, so nothing in the workspace can
    /// populate it today, and a name this build cannot derive is a name it
    /// cannot check.
    #[must_use]
    pub fn of(doi: Option<&str>) -> Self {
        match doi {
            None => Self::NoDoi,
            Some(doi) => match classify_publisher(doi) {
                PublisherVerdict::Known { publisher, .. } => {
                    Self::Classified(publisher.label().to_string())
                }
                PublisherVerdict::Unknown { prefix } => Self::Unclassified(prefix),
            },
        }
    }

    /// The publisher name, or `None` when the registry could not classify it.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Classified(label) => Some(label),
            Self::Unclassified(_) | Self::NoDoi => None,
        }
    }

    /// What a group heading may print. Never a publisher name the registry did
    /// not produce.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Classified(label) => label.clone(),
            Self::Unclassified(prefix) => {
                format!("unclassified (registrant prefix 10.{prefix})")
            }
            Self::NoDoi => "no DOI on record".to_string(),
        }
    }

    /// `RouteVerdict::note()`, or `None` when nothing was evaluated.
    ///
    /// `doi` is the representative work's DOI. Every work in a group has the
    /// same [`PublisherKey`], and the registry's TDM answer is a property of
    /// the publisher rather than of the suffix, so one representative decides
    /// the note for the whole group.
    #[must_use]
    pub fn route_note(&self, status: &str, doi: Option<&str>) -> Option<String> {
        match self {
            Self::Classified(_) => classified_route_note(status, doi),
            Self::Unclassified(prefix) => Some(
                RouteVerdict::PublisherUnknown {
                    prefix: prefix.clone(),
                }
                .note(),
            ),
            // The registry has no spelling for "no DOI" —
            // `classify_publisher("")` reports `<not-a-doi>`, which would print
            // a registrant prefix that does not exist — so this line is written
            // here and says only what was established.
            Self::NoDoi => Some(
                "no DOI on record, so no publisher was classified and no route was evaluated"
                    .to_string(),
            ),
        }
    }
}

/// One `(action group, publisher)` bucket of `action_list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionGroup {
    /// ADR-007 §2's action wording for this status.
    pub action: &'static str,
    /// The status every entry in this group carries. One action maps from
    /// exactly one status, so this pair with [`Self::publisher`] is the ADR's
    /// grouping key.
    pub status: &'static str,
    /// Which publisher these works belong to, and whether that is a name or
    /// only a reason there is none.
    pub publisher: PublisherKey,
    /// `RouteVerdict::note()` for the group, when a verdict is actually
    /// established, or `None` when nothing was evaluated.
    pub route_note: Option<String>,
    /// Every member entry, so a consumer gets `hint_url` and `drop_path`
    /// without a second query — ADR-007 §2 asks `action_list` to carry both.
    pub entries: Vec<MissingEntry>,
}

impl ActionGroup {
    /// How many missing entries this group accounts for.
    #[must_use]
    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

/// A status with no human action, and where its entries went instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DeferredGroup {
    pub status: &'static str,
    /// ADR-007 §2's meaning for the status.
    pub meaning: &'static str,
    /// What `action_list` does with these rows.
    pub goes_to: &'static str,
    pub count: usize,
}

/// `action_list`: the grouped view of one [`CoverageReport`].
///
/// Holds the report it was built from, so a CLI can render either projection
/// from a single value and the two cannot be computed from different reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionList {
    pub report: CoverageReport,
    /// Human-actionable groups, in ADR-007 §2's status order.
    pub groups: Vec<ActionGroup>,
    /// Statuses the ADR marks "—", with their counts.
    pub deferred: Vec<DeferredGroup>,
}

impl ActionList {
    /// Group `report.entries` by (action group, publisher).
    ///
    /// Every entry lands in exactly one bucket: a group when its status has a
    /// human action, a deferred bucket when it does not, and — for a status
    /// this build does not know, which the schema's CHECK forbids but which a
    /// future migration could introduce — a deferred bucket of its own rather
    /// than nothing at all. That exhaustiveness *is* the acceptance criterion:
    /// see [`Self::accounted`].
    #[must_use]
    pub fn from_report(report: CoverageReport) -> Self {
        // Insertion-ordered buckets keyed by the ADR's grouping key, then
        // sorted into §2's status order so the list reads the way the
        // vocabulary does rather than by insertion accident.
        let mut groups: Vec<(GroupKey, Vec<MissingEntry>)> = Vec::new();
        let mut deferred_counts: BTreeMap<String, usize> = BTreeMap::new();

        for entry in &report.entries {
            let Some(vocab) = status_vocab(&entry.status) else {
                // Never drop the row: a lost entry would make `action_list`
                // under-count `coverage`, which is the disagreement ADR-007 §2
                // forbids.
                tracing::warn!(
                    status = %entry.status,
                    paper_id = %entry.paper_id,
                    "acquisition_state carries a status this build does not know; \
                     reported separately so it is never lost"
                );
                *deferred_counts.entry(entry.status.clone()).or_insert(0) += 1;
                continue;
            };
            let Some(action) = vocab.action else {
                *deferred_counts.entry(entry.status.clone()).or_insert(0) += 1;
                continue;
            };
            let doi = report.doi_of(&entry.paper_id).map(str::to_string);
            let key = GroupKey {
                action,
                status: vocab.status,
                publisher: PublisherKey::of(doi.as_deref()),
                doi,
            };
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, bucket)) => bucket.push(entry.clone()),
                None => groups.push((key, vec![entry.clone()])),
            }
        }
        groups.sort_by(|(a, _), (b, _)| {
            vocab_index(a.status)
                .cmp(&vocab_index(b.status))
                .then_with(|| a.publisher.cmp(&b.publisher))
        });

        // An unrecognised status has no `StatusVocab` row, so it is counted
        // apart and labelled with the only two things that can honestly be said
        // about it: that this build does not know it, and that its count is
        // still here.
        let mut deferred: Vec<DeferredGroup> = Vec::new();
        let mut unknown_count = 0usize;
        for (status, count) in &deferred_counts {
            match status_vocab(status) {
                Some(vocab) => deferred.push(DeferredGroup {
                    status: vocab.status,
                    meaning: vocab.meaning,
                    goes_to: vocab.goes_to,
                    count: *count,
                }),
                None => unknown_count += count,
            }
        }
        if unknown_count > 0 {
            deferred.push(DeferredGroup {
                status: "unknown",
                meaning: "a status this build of scitadel does not know",
                goes_to: "no action: reported so the count still adds up",
                count: unknown_count,
            });
        }
        deferred.sort_by_key(|d| vocab_index(d.status));

        let list = Self {
            report,
            groups: groups
                .into_iter()
                .map(|(key, entries)| {
                    let route_note = key.publisher.route_note(key.status, key.doi.as_deref());
                    ActionGroup {
                        action: key.action,
                        status: key.status,
                        publisher: key.publisher,
                        route_note,
                        entries,
                    }
                })
                .collect(),
            deferred,
        };
        debug_assert_eq!(
            list.accounted(),
            list.report.missing_total(),
            "action_list must account for every missing entry exactly once"
        );
        list
    }

    /// Entries a human can act on.
    #[must_use]
    pub fn human_total(&self) -> usize {
        self.groups.iter().map(ActionGroup::count).sum()
    }

    /// Every missing entry, counted from the buckets.
    ///
    /// This is the number ADR-007 §2 says must equal the missing totals in
    /// `coverage`, and it is a sum over a partition of
    /// [`CoverageReport::entries`] rather than a second count of them.
    #[must_use]
    pub fn accounted(&self) -> usize {
        self.human_total() + self.deferred.iter().map(|d| d.count).sum::<usize>()
    }

    /// Entries left out of the human list.
    #[must_use]
    pub fn deferred_total(&self) -> usize {
        self.deferred.iter().map(|d| d.count).sum()
    }

    /// `coverage`'s projection: the report this grouped view was built from,
    /// optionally narrowed to one want `kind`.
    ///
    /// The reason a caller reaches for the [`ActionList`] rather than for the
    /// bare report: `self.accounted()` is the sum over the partition of
    /// `self.report.entries`, so one value yields both the per-kind and
    /// per-status tables *and* the number `action_list` has to match.
    pub fn coverage(&self, kind: Option<&str>) -> Result<CoverageReport, UnknownWantKind> {
        self.report.clone().filtered(kind)
    }
}

/// One bucket's identity: ADR-007 §2's (action group, publisher).
///
/// `doi` is *not* part of the identity — two works from one publisher share a
/// group and their DOIs differ — so equality is written out rather than
/// derived, which keeps a future field from silently splitting a group in two.
#[derive(Debug, Clone)]
struct GroupKey {
    action: &'static str,
    status: &'static str,
    publisher: PublisherKey,
    doi: Option<String>,
}

impl PartialEq for GroupKey {
    fn eq(&self, other: &Self) -> bool {
        self.action == other.action
            && self.status == other.status
            && self.publisher == other.publisher
    }
}

impl Eq for GroupKey {}

/// Position of `status` in ADR-007 §2, for the stable ordering.
fn vocab_index(status: &str) -> usize {
    ALL_STATUSES
        .iter()
        .position(|v| v.status == status)
        .unwrap_or(usize::MAX)
}

/// The route verdict for a **classified** publisher's group, or `None` when nothing
/// was evaluated.
///
/// `NoTdmRouteAvailable` is reachable only from the classified-and-no-TDM arm,
/// which is the whole point: an unclassified prefix takes the `Unknown` arm and
/// gets a note that says a route was never looked for.
///
/// The credential answer comes from the *status*, not from a fresh keychain
/// probe. `tdm_available` was written by something that checked; re-reading the
/// credential store from a reporting command would prompt for a keychain unlock
/// to produce a report, and could contradict the ladder that wrote the row. Any
/// other status alongside a TDM-capable publisher reports no verdict at all,
/// because "a route exists" plus "some other status" establishes nothing about
/// the credential.
fn classified_route_note(status: &str, doi: Option<&str>) -> Option<String> {
    let verdict = match classify_publisher(doi?) {
        PublisherVerdict::Unknown { prefix } => RouteVerdict::PublisherUnknown { prefix },
        PublisherVerdict::Known {
            publisher,
            tdm: None,
            ..
        } => RouteVerdict::NoTdmRouteAvailable { publisher },
        PublisherVerdict::Known {
            publisher,
            tdm: Some(route),
            ..
        } => match status {
            "tdm_available" => RouteVerdict::TdmAvailable { publisher, route },
            "tdm_key_missing" => RouteVerdict::TdmKeyMissing { publisher, route },
            _ => return None,
        },
    };
    Some(verdict.note())
}

/// One `acquisition_state` row.
#[derive(Debug, Clone)]
struct WantRow {
    paper_id: String,
    kind: String,
    locator: String,
    wanted_version: String,
    status: String,
    reason: Option<String>,
    publisher: Option<String>,
    hint_url: Option<String>,
    drop_path: Option<String>,
}

/// Read the report. `kind` filters it; see [`CoverageReport::filtered`].
///
/// # Errors
///
/// `DbError` if SQLite fails. Nothing is refused for content: a work with no
/// want row and no artefact is reported as untracked, never dropped.
pub fn coverage_report(
    conn: &Connection,
    kind: Option<&str>,
) -> Result<CoverageReport, CoverageError> {
    let wants = read_wants(conn)?;
    let dois = read_dois(conn)?;
    let held = read_held_artefacts(conn)?;
    let works_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM papers", [], |r| r.get(0))
        .map_err(DbError::from)?;

    let wanted_papers: HashSet<&str> = wants.iter().map(|w| w.paper_id.as_str()).collect();
    let works_in_scope = wanted_papers.len();
    let held_untracked_works = held
        .keys()
        .filter(|paper_id| !wanted_papers.contains(paper_id.as_str()))
        .count();

    let mut report = CoverageReport {
        entries: Vec::new(),
        by_kind: empty_kind_totals(),
        by_status: empty_status_totals(),
        held_at_another_version: 0,
        works_total: usize::try_from(works_total).unwrap_or(usize::MAX),
        works_in_scope,
        untracked_works: usize::try_from(works_total)
            .unwrap_or(0)
            .saturating_sub(works_in_scope)
            .saturating_sub(held_untracked_works),
        held_untracked_works,
        work_dois: BTreeMap::new(),
        version_mismatches: BTreeSet::new(),
        // Propagated rather than defaulted: a report that printed "no overrides"
        // because the read failed would be indistinguishable from one that
        // printed it because there are none — which is exactly the silence
        // `scitadel override-identity` exists to avoid.
        identity_overrides: crate::sqlite::identity::identity_overrides(conn)?,
    };

    for want in wants {
        let empty = Vec::new();
        let candidates = held.get(&want.paper_id).unwrap_or(&empty);
        let satisfied = is_satisfied(&want.kind, &want.locator, &want.wanted_version, candidates);
        let total = report.by_kind.entry(want.kind.clone()).or_default();
        total.wanted += 1;
        if satisfied == Satisfaction::Held {
            total.held += 1;
            continue;
        }
        if satisfied == Satisfaction::HeldAtAnotherVersion {
            report.held_at_another_version += 1;
            report.version_mismatches.insert((
                want.paper_id.clone(),
                want.kind.clone(),
                want.locator.clone(),
            ));
        }
        report
            .by_kind
            .get_mut(&want.kind)
            .expect("just inserted")
            .missing += 1;
        *report.by_status.entry(want.status.clone()).or_insert(0) += 1;
        if let Some(doi) = dois.get(&want.paper_id) {
            report.work_dois.insert(want.paper_id.clone(), doi.clone());
        }
        report.entries.push(MissingEntry {
            paper_id: want.paper_id,
            kind: want.kind,
            locator: want.locator,
            wanted_version: want.wanted_version,
            status: want.status,
            reason: want.reason,
            publisher: want.publisher,
            hint_url: want.hint_url,
            drop_path: want.drop_path,
        });
    }
    // `(kind, locator, paper_id)` so two runs over one library print alike.
    report.entries.sort_by(|a, b| {
        (&a.kind, &a.locator, &a.paper_id).cmp(&(&b.kind, &b.locator, &b.paper_id))
    });

    Ok(report.filtered(kind)?)
}

fn read_wants(conn: &Connection) -> Result<Vec<WantRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT paper_id, kind, locator, wanted_version, status, reason,
                publisher, hint_url, drop_path
         FROM acquisition_state",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(WantRow {
                paper_id: row.get(0)?,
                kind: row.get(1)?,
                locator: row.get(2)?,
                wanted_version: row.get(3)?,
                status: row.get(4)?,
                reason: row.get(5)?,
                publisher: row.get(6)?,
                hint_url: row.get(7)?,
                drop_path: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every work's DOI, for publisher classification (#261).
///
/// Works with no DOI simply do not appear: there is nothing to classify a
/// registrant prefix from, and `ActionGroup` says exactly that.
fn read_dois(conn: &Connection) -> Result<HashMap<String, String>, DbError> {
    let mut stmt =
        conn.prepare("SELECT id, doi FROM papers WHERE doi IS NOT NULL AND doi <> ''")?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().collect())
}

/// Every artefact row that could close a want, keyed by work.
///
/// `access_status = 'full_text'` is ADR-007 §1's gate. `missing_on_disk = 0`
/// is this module's addition of the same kind: the column means "the path is
/// recorded but we hold no usable bytes" (see `sqlite::acquisition`), so
/// counting such a row would let a deleted file read as a held full text.
///
/// `sha256 IS NULL` is deliberately *not* filtered: a figure reference with no
/// blob is a real artefact (ADR-007 §1 "Artefact rules"), and ADR-007 §1 "Have"
/// gates on `access_status` alone.
fn read_held_artefacts(conn: &Connection) -> Result<HashMap<String, Vec<HeldArtefact>>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT paper_id, kind, locator, version, route FROM artefacts
         WHERE access_status = 'full_text' AND missing_on_disk = 0",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                HeldArtefact {
                    kind: row.get(1)?,
                    locator: row.get(2)?,
                    version: row.get(3)?,
                    route: row.get(4)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut by_paper: HashMap<String, Vec<HeldArtefact>> = HashMap::new();
    for (paper_id, artefact) in rows {
        by_paper.entry(paper_id).or_default().push(artefact);
    }
    Ok(by_paper)
}

impl Database {
    /// ADR-007 §1 / §2: derive every wanted artefact we do not hold.
    ///
    /// `kind` restricts the report to one want kind; see
    /// [`CoverageReport::filtered`].
    pub fn coverage_report(&self, kind: Option<&str>) -> Result<CoverageReport, CoverageError> {
        let conn = self.pool.get().map_err(DbError::from)?;
        coverage_report(&conn, kind)
    }

    /// ADR-007 §2: the same report, grouped for a human.
    pub fn action_list(&self) -> Result<ActionList, CoverageError> {
        Ok(self.coverage_report(None)?.action_list())
    }

    /// Every artefact ADR-007 §1 says this work holds as its full text, best first
    /// — the answer `find_cached_file` and `read_paper` open.
    ///
    /// `wanted_version` is the caller's own question; see [`ANY_VERSION`] for the
    /// one a reader wants.
    ///
    /// # Errors
    ///
    /// `DbError` if SQLite fails. Nothing is refused for content: a work with
    /// nothing held is an empty vector, not an error.
    pub fn held_fulltext_artefacts(
        &self,
        paper_id: &str,
        wanted_version: &str,
    ) -> Result<Vec<crate::sqlite::artefacts::ArtefactRow>, DbError> {
        let conn = self.pool.get().map_err(DbError::from)?;
        held_fulltext_artefacts(&conn, paper_id, wanted_version)
    }

    /// The derived download state of each work in `paper_ids` — what a reader's
    /// state column shows. A work with nothing recorded is absent from the map.
    ///
    /// # Errors
    ///
    /// `DbError` if SQLite fails.
    pub fn download_states(
        &self,
        paper_ids: &[String],
    ) -> Result<HashMap<String, DownloadState>, DbError> {
        let conn = self.pool.get().map_err(DbError::from)?;
        download_states(&conn, paper_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::artefacts::{
        ACCESS_BASIS_MANUAL, ArtefactWrite, BlobWrite, ROUTE_IMPORT_FLAT, StateWrite,
        VERSION_UNKNOWN, WriteMode, write_acquisition_states, write_artefacts,
    };
    use crate::sqlite::{Database, ROUTE_LEGACY};

    const NOW: &str = "2026-01-01T00:00:00+00:00";

    /// A migrated file-backed database with three works: an RSC paper nobody has
    /// touched, a bioRxiv preprint, and a work with no DOI at all.
    ///
    /// The `TempDir` is returned alongside because the pool opens connections
    /// lazily: dropping the directory mid-test would let a later `conn()`
    /// create a fresh, empty database file.
    fn open() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("scitadel.db")).unwrap();
        db.migrate().unwrap();
        add_paper(&db, "p-wanted", Some("10.1039/d0nr01234a"));
        add_paper(&db, "p-held", Some("10.1101/2025.06.14.659707"));
        add_paper(&db, "p-untouched", None);
        (dir, db)
    }

    /// `papers.doi` carries a partial unique index (migration 001), so a fixture
    /// work needs its own DOI even when it shares a publisher.
    fn add_paper(db: &Database, id: &str, doi: Option<&str>) {
        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO papers (id, title, authors, doi, created_at, updated_at)
             VALUES (?1, 't', '[]', ?2, ?3, ?3)",
            rusqlite::params![id, doi, NOW],
        )
        .unwrap();
    }

    /// `n` works of one publisher, for the tests that need several.
    fn add_papers(db: &Database, prefix: &str, registrant: &str, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| {
                let id = format!("{prefix}{i}");
                add_paper(db, &id, Some(&format!("10.{registrant}/work-{i}")));
                id
            })
            .collect()
    }

    fn set_doi(db: &Database, id: &str, doi: &str) {
        let conn = db.conn().unwrap();
        conn.execute(
            "UPDATE papers SET doi = ?1 WHERE id = ?2",
            rusqlite::params![doi, id],
        )
        .unwrap();
    }

    fn state(
        paper_id: &str,
        kind: &str,
        locator: &str,
        wanted_version: &str,
        status: &str,
    ) -> StateWrite {
        StateWrite {
            paper_id: paper_id.into(),
            kind: kind.into(),
            locator: locator.into(),
            wanted_version: wanted_version.into(),
            status: status.into(),
            reason: None,
            // No publisher and no retry time: this helper is about the *want*,
            // and a fixture that stored a publisher would let a test pass on a
            // publisher it never meant to exercise.
            publisher: None,
            hint_url: None,
            drop_path: None,
            next_attempt_at: None,
            updated_at: NOW.into(),
        }
    }

    /// A `fulltext` want for the version of record, which is what the ADR's
    /// default column value says and what most tests want.
    fn want(db: &Database, paper_id: &str, kind: &str, status: &str) {
        want_at(db, paper_id, kind, "", "vor", status);
    }

    fn want_at(
        db: &Database,
        paper_id: &str,
        kind: &str,
        locator: &str,
        wanted_version: &str,
        status: &str,
    ) {
        let mut conn = db.conn().unwrap();
        write_acquisition_states(
            &mut conn,
            &[state(paper_id, kind, locator, wanted_version, status)],
        )
        .unwrap();
    }

    /// One artefact row, with the `blobs` row its `sha256` references. An
    /// artefact that claims content cannot leave the blob out: `PRAGMA
    /// foreign_keys` is on, so the artefacts insert is what refuses.
    fn artefact_row(
        paper_id: &str,
        kind: &str,
        locator: &str,
        version: &str,
        route: &str,
        access_status: &str,
        missing_on_disk: bool,
    ) -> ArtefactWrite {
        let sha = format!("{paper_id}-{kind}-{locator}-{version}");
        ArtefactWrite {
            id: String::new(),
            paper_id: paper_id.into(),
            kind: kind.into(),
            version: version.into(),
            locator: locator.into(),
            // A vanished file keeps the row but has no bytes, so no hash and no
            // blob — which is exactly the case the derivation must not count.
            sha256: (!missing_on_disk).then(|| sha.clone()),
            format: Some("pdf".into()),
            access_status: access_status.into(),
            route: route.into(),
            access_basis: ACCESS_BASIS_MANUAL.into(),
            label: None,
            caption: None,
            source_url: None,
            publisher: None,
            publisher_note: None,
            license_url: None,
            license_content_version: None,
            license_start: None,
            license_source: None,
            imported_from: None,
            retrieved_at: NOW.into(),
            missing_on_disk,
            blob: (!missing_on_disk).then(|| BlobWrite {
                sha256: sha,
                bytes: 4,
                mime: "application/pdf".into(),
                rel_path: "blobs/aa/artefact.pdf".into(),
                created_at: NOW.into(),
            }),
        }
    }

    /// A held `full_text` artefact — the case that closes a want.
    fn artefact(db: &Database, paper_id: &str, kind: &str, version: &str, route: &str) {
        write_artefact(
            db,
            artefact_row(paper_id, kind, "", version, route, "full_text", false),
        );
    }

    fn write_artefact(db: &Database, row: ArtefactWrite) {
        let mut conn = db.conn().unwrap();
        write_artefacts(&mut conn, &[row], WriteMode::Reconcile).unwrap();
    }

    // ---------- ADR-007 §1 "Have", read by a reader rather than a report ----------

    /// The one test that makes "reuse this module's derivation" true rather than
    /// aspirational: the report's bulk SQL and the reader's row filter are two
    /// implementations of ADR-007 §1's gate, so they are run against a matrix of
    /// rows that disagree on every clause and required to agree on every one.
    ///
    /// Each row below differs from a held full text in exactly one way, and each
    /// case asserts three things that must not be allowed to drift apart: the
    /// report's `held` count for the work, the rows [`held_fulltext_artefacts`]
    /// returns, and [`download_states`]' verdict.
    #[test]
    fn the_report_and_a_reader_agree_on_which_artefact_is_the_full_text() {
        // (paper_id, note, what one clause does)
        let cases: [(&str, &str, ArtefactWrite); 8] = [
            (
                "p-plain",
                "a plain held PDF",
                artefact_row(
                    "p-plain",
                    "fulltext_pdf",
                    "",
                    "vor",
                    "publisher",
                    "full_text",
                    false,
                ),
            ),
            (
                "p-abstract",
                "access_status is not full_text",
                artefact_row(
                    "p-abstract",
                    "fulltext_pdf",
                    "",
                    "vor",
                    "publisher",
                    "abstract",
                    false,
                ),
            ),
            (
                "p-paywall",
                "a paywall stub is bytes, not the paper",
                artefact_row(
                    "p-paywall",
                    "fulltext_html",
                    "",
                    "vor",
                    "publisher",
                    "paywall",
                    false,
                ),
            ),
            (
                "p-vanished",
                "the row is there and the bytes are not",
                artefact_row(
                    "p-vanished",
                    "fulltext_pdf",
                    "",
                    "vor",
                    "publisher",
                    "full_text",
                    true,
                ),
            ),
            (
                "p-si",
                "an SI is not a full text",
                artefact_row(
                    "p-si",
                    "si",
                    "table-1",
                    "vor",
                    "publisher",
                    "full_text",
                    false,
                ),
            ),
            (
                "p-figure",
                "a figure reference is not a full text",
                artefact_row(
                    "p-figure",
                    "figure",
                    "fig-2",
                    "vor",
                    "publisher",
                    "full_text",
                    false,
                ),
            ),
            (
                "p-preprint",
                "a preprint is held, but not at `vor`",
                artefact_row(
                    "p-preprint",
                    "fulltext_pdf",
                    "",
                    "preprint",
                    "arxiv",
                    "full_text",
                    false,
                ),
            ),
            (
                "p-legacy",
                "a legacy file is `unknown`, and that is trusted",
                artefact_row(
                    "p-legacy",
                    "fulltext_pdf",
                    "",
                    "unknown",
                    ROUTE_LEGACY,
                    "full_text",
                    false,
                ),
            ),
        ];
        let (_dir, db) = open();
        for (paper_id, _, row) in &cases {
            add_paper(&db, paper_id, None);
            // Every work wants the version of record, which is the want the report
            // is built around and the one that distinguishes a preprint.
            want(&db, paper_id, "fulltext", "pending");
            write_artefact(&db, row.clone());
        }

        let report = db.coverage_report(None).unwrap();

        for (paper_id, note, row) in &cases {
            let report_holds = report.entries.iter().all(|e| e.paper_id != *paper_id);
            let held = db.held_fulltext_artefacts(paper_id, "vor").unwrap();
            // `ArtefactWrite.id` is derived at write time, so the identity to
            // compare is the UNIQUE key rather than a caller-supplied id.
            let matches_row = held.len() == 1
                && held[0].kind == row.kind
                && held[0].version == row.version
                && held[0].locator == row.locator;

            // One table, one answer: the report, the reader and the column all say
            // the same thing about this row, or this test fails.
            let expect_held = matches!(*paper_id, "p-plain" | "p-legacy");
            assert_eq!(
                report_holds, expect_held,
                "{paper_id} ({note}): the report and the reader must agree"
            );
            assert_eq!(
                matches_row, expect_held,
                "{paper_id} ({note}): the reader returns the row the report counted"
            );
        }

        // `any` is a reader's question, not a want's: the preprint is a file we can
        // open, so a reader takes it while the `vor` want stays a version mismatch.
        let for_reader = db
            .held_fulltext_artefacts("p-preprint", ANY_VERSION)
            .unwrap();
        assert_eq!(for_reader.len(), 1, "a reader opens the preprint we hold");
        assert!(
            report
                .entries
                .iter()
                .any(|e| e.paper_id == "p-preprint" && e.wanted_version == "vor"),
            "while the report still counts the `vor` want as missing"
        );
        assert_eq!(
            report.held_at_another_version, 1,
            "and names it a version mismatch rather than an absence"
        );
    }

    /// A work holding several full texts: the version of record first, then the
    /// serialisation a reader can actually open. The old `find_cached_file`
    /// preferred `.pdf` and nothing else, so this pins that behaviour *inside* the
    /// derivation rather than in a caller's loop.
    #[test]
    fn a_reader_is_offered_the_version_of_record_before_the_preprint() {
        let (_dir, db) = open();
        add_paper(&db, "p-many", None);
        for (kind, version, route) in [
            ("fulltext_html", "vor", "publisher"),
            ("fulltext_pdf", "preprint", "arxiv"),
            ("fulltext_pdf", "vor", "publisher"),
            ("fulltext_xml", "am", "europepmc"),
        ] {
            write_artefact(
                &db,
                artefact_row("p-many", kind, "", version, route, "full_text", false),
            );
        }
        let held = db.held_fulltext_artefacts("p-many", ANY_VERSION).unwrap();
        let order: Vec<(&str, &str)> = held
            .iter()
            .map(|r| (r.version.as_str(), r.kind.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![
                ("vor", "fulltext_pdf"),
                ("vor", "fulltext_html"),
                ("am", "fulltext_xml"),
                ("preprint", "fulltext_pdf"),
            ],
            "VoR first, then a serialisation a reader can open; a held VoR PDF \
             beats an HTML VoR because the extractor and the OS viewer both take it"
        );
    }

    /// The four shapes a state column shows, derived. `pending` is excluded from
    /// `Gap` on purpose: it is the only status that means "never tried", so a work
    /// in the `acquire` queue is not a work whose download failed.
    #[test]
    fn the_derived_download_state_of_every_shape() {
        let (_dir, db) = open();
        for id in [
            "p-full", "p-stub", "p-gone", "p-failed", "p-queued", "p-silent",
        ] {
            add_paper(&db, id, None);
        }
        write_artefact(
            &db,
            artefact_row(
                "p-full",
                "fulltext_pdf",
                "",
                "vor",
                "publisher",
                "full_text",
                false,
            ),
        );
        write_artefact(
            &db,
            artefact_row(
                "p-stub",
                "fulltext_html",
                "",
                "vor",
                "publisher",
                "paywall",
                false,
            ),
        );
        write_artefact(
            &db,
            artefact_row(
                "p-gone",
                "fulltext_pdf",
                "",
                "vor",
                "publisher",
                "full_text",
                true,
            ),
        );
        want(&db, "p-failed", "fulltext", "error");
        want(&db, "p-queued", "fulltext", "pending");

        let ids: Vec<String> = [
            "p-full", "p-stub", "p-gone", "p-failed", "p-queued", "p-silent",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let states = db.download_states(&ids).unwrap();

        assert_eq!(states.get("p-full"), Some(&DownloadState::FullText));
        assert_eq!(states.get("p-stub"), Some(&DownloadState::NotFullText));
        assert_eq!(states.get("p-gone"), Some(&DownloadState::Missing));
        assert_eq!(states.get("p-failed"), Some(&DownloadState::Gap));
        assert_eq!(
            states.get("p-queued"),
            None,
            "`pending` is the only status that means \"never tried\", and a work \
             waiting in the `acquire` queue has concluded nothing — so it is blank \
             rather than a failure"
        );
        assert_eq!(
            states.get("p-silent"),
            None,
            "a work with nothing recorded is absent, so a table keeps its own \
             \"not tracked\" default rather than this function guessing it"
        );

        // A full text beats a gap row, in whichever order the rows come back: the
        // file is on disk, which is what the column is for.
        want(&db, "p-full", "fulltext", "needs_ill");
        let after = db.download_states(&["p-full".to_string()]).unwrap();
        assert_eq!(after.get("p-full"), Some(&DownloadState::FullText));
    }

    /// Holding the full text and holding a stub for the same work is possible — a
    /// paywalled landing page first, then a real PDF from an OA route — and the
    /// column must not report the stub just because it is also there.
    #[test]
    fn a_stub_beside_a_full_text_does_not_downgrade_the_answer() {
        let (_dir, db) = open();
        add_paper(&db, "p-both", None);
        write_artefact(
            &db,
            artefact_row(
                "p-both",
                "fulltext_html",
                "",
                "unknown",
                "publisher",
                "paywall",
                false,
            ),
        );
        write_artefact(
            &db,
            artefact_row(
                "p-both",
                "fulltext_pdf",
                "",
                "vor",
                "unpaywall",
                "full_text",
                false,
            ),
        );
        let states = db.download_states(&["p-both".to_string()]).unwrap();
        assert_eq!(states.get("p-both"), Some(&DownloadState::FullText));
    }

    // ---------- ADR-007 §1 "Have" ----------

    /// A want with nothing behind it is missing. The status is `pending` in both
    /// halves of this pair — nothing here reads it, which is the point: "have"
    /// is derived, never asserted.
    #[test]
    fn a_work_with_no_artefact_and_a_want_is_missing() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "pending");
        let report = db.coverage_report(None).unwrap();
        assert_eq!(report.missing_total(), 1, "{report:?}");
        let entry = &report.entries[0];
        assert_eq!(entry.paper_id, "p-wanted");
        assert_eq!(entry.kind, "fulltext");
        assert_eq!(entry.wanted_version, "vor");
        assert_eq!(entry.status, "pending");
        assert_eq!(
            report.by_kind["fulltext"],
            KindTotal {
                wanted: 1,
                missing: 1,
                held: 0
            }
        );
        assert_eq!(
            report.held_at_another_version, 0,
            "we hold nothing at all, so this is not a version mismatch"
        );
    }

    #[test]
    fn the_same_work_with_a_full_text_artefact_is_not() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "pending");
        artefact(&db, "p-wanted", "fulltext_pdf", "vor", "publisher");
        let report = db.coverage_report(None).unwrap();
        assert!(
            report.entries.is_empty(),
            "a held full text closes the full-text want: {report:?}"
        );
        assert_eq!(
            report.by_kind["fulltext"],
            KindTotal {
                wanted: 1,
                missing: 0,
                held: 1
            }
        );
        assert_eq!(report.missing_total(), 0);
        assert_eq!(report.held_total(), 1);
    }

    /// Any serialisation closes the bare `fulltext` want.
    #[test]
    fn the_fulltext_want_accepts_any_serialisation() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "pending");
        artefact(&db, "p-wanted", "fulltext_xml", "vor", "europepmc");
        let report = db.coverage_report(None).unwrap();
        assert_eq!(report.missing_total(), 0, "{report:?}");
    }

    /// An `access_status` that is not `full_text` does not close the want: a
    /// paywalled stub on disk is not the full text, which is why the column
    /// exists.
    #[test]
    fn an_artefact_that_is_not_full_text_does_not_close_the_want() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "pending");
        write_artefact(
            &db,
            artefact_row(
                "p-wanted",
                "fulltext_pdf",
                "",
                "vor",
                ROUTE_LEGACY,
                "paywall",
                false,
            ),
        );
        let report = db.coverage_report(None).unwrap();
        assert_eq!(
            report.missing_total(),
            1,
            "a paywalled stub is not a full text: {report:?}"
        );
        assert_eq!(report.by_kind["fulltext"].held, 0);
    }

    /// `missing_on_disk = 1` means "the path is recorded but we hold no usable
    /// bytes". Counting it would let a deleted file read as a held full text —
    /// the same class of overclaim as the publisher one, one column over.
    #[test]
    fn a_artefact_that_is_missing_on_disk_does_not_close_the_want() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "pending");
        write_artefact(
            &db,
            artefact_row(
                "p-wanted",
                "fulltext_pdf",
                "",
                "vor",
                ROUTE_LEGACY,
                "full_text",
                true,
            ),
        );
        let report = db.coverage_report(None).unwrap();
        assert_eq!(report.missing_total(), 1, "{report:?}");
        assert_eq!(report.by_kind["fulltext"].held, 0);
    }

    /// ADR-007 §1's version rules as a table: `any` takes everything, `vor`
    /// takes `vor` (plus `unknown` only from the two routes that predate version
    /// tracking), `am` and `preprint` take themselves, and nothing else does.
    #[test]
    fn a_wanted_version_is_satisfied_only_by_a_matching_artefact_version() {
        let cases = [
            // wanted, held, route, satisfied
            ("vor", "vor", "publisher", true),
            ("vor", "am", "europepmc", false),
            ("vor", "preprint", "arxiv", false),
            ("vor", VERSION_UNKNOWN, "openalex", false),
            ("vor", VERSION_UNKNOWN, "unpaywall", false),
            ("vor", VERSION_UNKNOWN, "manual", false),
            ("vor", VERSION_UNKNOWN, "manual_url", false),
            ("vor", VERSION_UNKNOWN, ROUTE_LEGACY, true),
            ("vor", VERSION_UNKNOWN, ROUTE_IMPORT_FLAT, true),
            ("am", "am", "europepmc", true),
            ("am", "vor", "publisher", false),
            ("am", "preprint", "arxiv", false),
            ("am", VERSION_UNKNOWN, ROUTE_LEGACY, false),
            ("preprint", "preprint", "biorxiv", true),
            ("preprint", "vor", "publisher", false),
            ("preprint", VERSION_UNKNOWN, ROUTE_LEGACY, false),
            ("any", "vor", "publisher", true),
            ("any", "am", "europepmc", true),
            ("any", "preprint", "arxiv", true),
            ("any", VERSION_UNKNOWN, "openalex", true),
            ("any", VERSION_UNKNOWN, ROUTE_LEGACY, true),
            // A `wanted_version` the schema never writes is satisfied by
            // nothing: unrecognised input must not read as `any`.
            ("nonsense", "vor", "publisher", false),
            ("", "vor", "publisher", false),
        ];
        for (wanted, held, route, expected) in cases {
            assert_eq!(
                version_satisfies(wanted, held, route),
                expected,
                "wanted {wanted:?}, held {held:?} from route {route}"
            );
        }
    }

    /// The same rules end to end, through the reader rather than the helper.
    /// One work per case: `(paper_id, kind, locator)` is the want's primary key,
    /// so five wants on one work would overwrite each other.
    #[test]
    fn the_reader_applies_the_version_rules_to_real_rows() {
        let (_dir, db) = open();
        let ids = add_papers(&db, "p-v", "1039", 5);
        // A preprint held against a VoR want; the same preprint against a
        // preprint want; an `unknown` S1 fetch against `any`, against `vor` from
        // a legacy row, and against `vor` from a route that never established
        // one.
        artefact(&db, &ids[0], "fulltext_pdf", "preprint", "arxiv");
        artefact(&db, &ids[1], "fulltext_pdf", "preprint", "arxiv");
        artefact(&db, &ids[2], "fulltext_pdf", VERSION_UNKNOWN, "openalex");
        artefact(&db, &ids[3], "fulltext_pdf", VERSION_UNKNOWN, ROUTE_LEGACY);
        artefact(&db, &ids[4], "fulltext_pdf", VERSION_UNKNOWN, "openalex");

        want_at(&db, &ids[0], "fulltext", "", "vor", "needs_ill");
        want_at(&db, &ids[1], "fulltext", "", "preprint", "pending");
        want_at(&db, &ids[2], "fulltext", "", "any", "pending");
        want_at(&db, &ids[3], "fulltext", "", "vor", "pending");
        want_at(&db, &ids[4], "fulltext", "", "vor", "pending");

        let report = db.coverage_report(None).unwrap();
        assert_eq!(
            report.missing_total(),
            2,
            "a preprint does not satisfy a VoR want, and an openalex `unknown` \
             is not trusted for one either: {report:?}"
        );
        assert_eq!(
            report
                .entries
                .iter()
                .map(|e| e.paper_id.as_str())
                .collect::<Vec<_>>(),
            vec![ids[0].as_str(), ids[4].as_str()],
            "entries are ordered by (kind, locator, paper_id), so two runs print alike"
        );
        assert_eq!(
            report.held_at_another_version, 2,
            "both are ADR-007 §2's `wrong_version` shape — we hold a full text \
             at a version the want does not accept, which is a different problem \
             from holding nothing: a preprint, and an S1 fetch whose version was \
             never established"
        );
    }

    /// A serialisation-specific want is not closed by a different serialisation,
    /// and a `table` want is not closed by a different locator.
    #[test]
    fn specific_kinds_match_the_exact_kind_and_locator() {
        let (_dir, db) = open();
        // The one want the artefact answers...
        want_at(&db, "p-wanted", "table", "table-1", "vor", "pending");
        // ...and three it must not: a different locator in a related kind, the
        // same locator in a different kind, and a serialisation we do not hold.
        want_at(&db, "p-wanted", "si", "table-2", "vor", "unavailable");
        want_at(&db, "p-wanted", "figure", "table-1", "any", "pending");
        want_at(&db, "p-wanted", "fulltext_pdf", "", "any", "pending");
        write_artefact(
            &db,
            artefact_row(
                "p-wanted",
                "table",
                "table-1",
                "vor",
                "publisher",
                "full_text",
                false,
            ),
        );
        artefact(&db, "p-wanted", "fulltext_html", "vor", "publisher");

        let report = db.coverage_report(None).unwrap();
        assert_eq!(
            report.missing_total(),
            3,
            "the locator mismatch, the kind mismatch and the serialisation \
             mismatch, and nothing else: {report:?}"
        );
        assert_eq!(
            report
                .entries
                .iter()
                .map(|e| e.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["figure", "fulltext_pdf", "si"]
        );
        assert_eq!(
            report.by_kind["table"],
            KindTotal {
                wanted: 1,
                missing: 0,
                held: 1
            },
            "the table want for table-1 is the one that was satisfied"
        );
    }

    /// #260's shape, restated: a work with no want row and no artefacts is not
    /// missing something. It has no recorded gap, so it is reported and not
    /// counted.
    #[test]
    fn a_work_with_no_want_row_and_no_artefacts_is_not_counted_as_missing() {
        let (_dir, db) = open();
        let report = db.coverage_report(None).unwrap();
        assert!(
            report.entries.iter().all(|e| e.paper_id != "p-untouched"),
            "{report:?}"
        );
        assert_eq!(report.missing_total(), 0);
        assert_eq!(report.wanted_total(), 0);
        assert_eq!(
            report.untracked_works, 3,
            "no work has a want at all, so all three are reported as untracked"
        );
        assert_eq!(report.held_untracked_works, 0);
        assert_eq!(report.works_total, 3);
        assert_eq!(report.works_in_scope, 0);
    }

    /// The mirror case: a work holding a file nobody asked for is counted as
    /// neither covered nor missing. Reporting it as covered would claim a want
    /// that was never stated.
    #[test]
    fn a_work_with_an_artefact_and_no_want_is_neither_covered_nor_missing() {
        let (_dir, db) = open();
        artefact(&db, "p-untouched", "fulltext_pdf", "vor", "openalex");
        let report = db.coverage_report(None).unwrap();
        assert_eq!(report.missing_total(), 0);
        assert_eq!(report.wanted_total(), 0, "no want was stated");
        assert_eq!(
            report.held_total(),
            0,
            "so nothing is 'held' against a want"
        );
        assert_eq!(report.held_untracked_works, 1);
        assert_eq!(report.untracked_works, 2);
    }

    /// Every status the ADR defines appears, including at zero — a reader has
    /// to be able to tell "no such papers" from "this category is not tracked".
    #[test]
    fn every_status_appears_even_with_a_zero_count() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "needs_ill");
        let report = db.coverage_report(None).unwrap();

        assert_eq!(report.by_status.len(), ALL_STATUSES.len());
        for vocab in ALL_STATUSES {
            let count = report
                .by_status
                .get(vocab.status)
                .unwrap_or_else(|| panic!("{} is missing from the report", vocab.status));
            if vocab.status == "needs_ill" {
                assert_eq!(*count, 1, "{}", vocab.status);
            } else {
                assert_eq!(*count, 0, "{} must still be reported at zero", vocab.status);
            }
        }
        // Kinds get the same treatment.
        assert_eq!(report.by_kind.len(), ALL_WANT_KINDS.len());
        for kind in ALL_WANT_KINDS {
            let total = report
                .by_kind
                .get(kind)
                .unwrap_or_else(|| panic!("{kind} missing from the report"));
            if *kind == *"fulltext" {
                assert_eq!(
                    *total,
                    KindTotal {
                        wanted: 1,
                        missing: 1,
                        held: 0
                    },
                    "{kind}"
                );
            } else {
                assert_eq!(
                    *total,
                    KindTotal::default(),
                    "{kind} must be reported at zero"
                );
            }
        }
    }

    /// The vocabulary in this module is the schema's, in both directions: every
    /// status here is writable, and the schema refuses one that is not. A status
    /// the schema allows that this table omits would be dropped from
    /// `action_list`, which is how the two commands would stop agreeing.
    #[test]
    fn the_status_vocabulary_is_exactly_the_schema_check() {
        let (_dir, db) = open();
        for vocab in ALL_STATUSES {
            let mut conn = db.conn().unwrap();
            write_acquisition_states(
                &mut conn,
                &[state("p-wanted", "fulltext", "", "vor", vocab.status)],
            )
            .unwrap_or_else(|e| panic!("{} should be writable: {e}", vocab.status));
        }
        let mut conn = db.conn().unwrap();
        let refused = write_acquisition_states(
            &mut conn,
            &[state("p-wanted", "fulltext", "", "vor", "not_a_status")],
        );
        assert!(
            refused.is_err(),
            "the schema must reject a status outside the vocabulary, or this \
             test proves nothing about the second direction"
        );
    }

    /// ADR-007 §2's row for every status exists, says something, and has exactly
    /// one destination. A status with no destination would be silently dropped
    /// from `action_list`'s accounting.
    #[test]
    fn every_status_says_where_its_rows_go() {
        for vocab in ALL_STATUSES {
            assert!(!vocab.meaning.is_empty(), "{}", vocab.status);
            assert!(!vocab.goes_to.is_empty(), "{}", vocab.status);
            if vocab.action.is_some() {
                assert_eq!(vocab.goes_to, "the human list", "{}", vocab.status);
            }
        }
        // The statuses ADR-007 §2 marks "—" are exactly the ones with no human
        // action, and none of them smuggles one in.
        let actionable: Vec<&str> = ALL_STATUSES
            .iter()
            .filter(|v| v.action.is_some())
            .map(|v| v.status)
            .collect();
        assert_eq!(
            actionable,
            vec![
                "tdm_key_missing",
                "needs_login",
                "not_entitled",
                "needs_authorisation",
                "needs_ill",
                "identity_mismatch",
                "wrong_version",
            ]
        );
    }

    // ---------- #261: never name an unclassified publisher ----------

    /// An unclassified registrant prefix produces no publisher and no verdict
    /// that says a route was evaluated.
    #[test]
    fn an_unclassified_prefix_produces_no_publisher_and_no_verdict() {
        let (_dir, db) = open();
        set_doi(&db, "p-wanted", "10.99999/some.suffix.12345");
        want(&db, "p-wanted", "fulltext", "needs_ill");
        let list = db.action_list().unwrap();

        assert_eq!(list.groups.len(), 1, "{list:?}");
        let group = &list.groups[0];
        assert_eq!(group.publisher.label(), None);
        assert!(matches!(group.publisher, PublisherKey::Unclassified(_)));
        let note = group.route_note.as_deref().expect("a note");
        assert!(!note.contains("no TDM route available"), "{note}");
        assert!(
            note.contains("not classified") && note.contains("99999"),
            "{note}"
        );
        for publisher in scitadel_core::publisher::ALL_PUBLISHERS {
            assert!(
                !note.contains(publisher.display_name()),
                "an unclassified publisher must not be named: {note}"
            );
        }
    }

    /// The converse: a classified publisher with no TDM API *is* an established
    /// negative, and this report is allowed to say so.
    #[test]
    fn a_classified_publisher_without_a_tdm_route_is_an_established_negative() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "needs_ill");
        let list = db.action_list().unwrap();
        let group = &list.groups[0];
        assert_eq!(group.publisher.label(), Some("rsc"));
        let note = group.route_note.as_deref().expect("a note");
        assert!(note.contains("no TDM route available"), "{note}");
        assert!(note.contains("Royal Society of Chemistry"), "{note}");
    }

    /// Only the two statuses that record a credential check may print a
    /// credential verdict. A TDM-capable publisher with any other status reports
    /// nothing, because the route's existence says nothing about the credential.
    #[test]
    fn a_tdm_credential_verdict_needs_a_status_that_checked_one() {
        let cases = [
            ("tdm_available", true),
            ("tdm_key_missing", true),
            ("needs_ill", false),
            ("pending", false),
            ("needs_login", false),
            ("identity_mismatch", false),
        ];
        for (status, expects_note) in cases {
            let note = classified_route_note(status, Some("10.1016/j.jneumeth.2005.09.009"));
            assert_eq!(note.is_some(), expects_note, "{status}: {note:?}");
            if let Some(note) = note {
                assert!(note.contains("Elsevier"), "{status}: {note}");
            }
        }
        // No DOI, no verdict — and not an invented one either.
        assert_eq!(classified_route_note("needs_ill", None), None);
        assert_eq!(
            classified_route_note("needs_ill", Some("not-a-doi")),
            Some(
                RouteVerdict::PublisherUnknown {
                    prefix: "<not-a-doi>".into()
                }
                .note()
            )
        );
    }

    /// A work with no DOI cannot be classified and no route was evaluated for it
    /// either — which the note has to say, rather than printing a
    /// publisher-shaped blank or a registrant prefix that does not exist.
    #[test]
    fn a_work_with_no_doi_reports_no_publisher_and_no_verdict() {
        let (_dir, db) = open();
        want(&db, "p-untouched", "fulltext", "needs_ill");
        let list = db.action_list().unwrap();
        let group = &list.groups[0];
        assert_eq!(group.publisher, PublisherKey::NoDoi);
        let note = group.route_note.as_deref().expect("a note");
        assert!(note.contains("no DOI on record"), "{note}");
        assert!(!note.contains("no TDM route available"), "{note}");
    }

    /// The stored `publisher` column is never used as a group key, so a stale or
    /// hand-written value cannot put a name in a heading — and it cannot make
    /// the heading contradict the verdict printed beside it either.
    #[test]
    fn a_stored_publisher_never_names_a_group() {
        let (_dir, db) = open();
        set_doi(&db, "p-wanted", "10.99999/some.suffix.12345");
        want(&db, "p-wanted", "fulltext", "needs_ill");
        {
            // `StateWrite` carries no `publisher` field (see `artefacts.rs`), so
            // nothing in S1 can set the column today; it is written directly
            // here to say what the reader does when something eventually does.
            let conn = db.conn().unwrap();
            conn.execute(
                "UPDATE acquisition_state SET publisher = 'elsevier' WHERE paper_id = 'p-wanted'",
                [],
            )
            .unwrap();
        }

        let report = db.coverage_report(None).unwrap();
        assert_eq!(
            report.entries[0].publisher.as_deref(),
            Some("elsevier"),
            "the stored value is still carried for machine consumers"
        );
        let list = report.action_list();
        assert_eq!(
            list.groups[0].publisher.label(),
            None,
            "but it names no group"
        );
        let note = list.groups[0].route_note.as_deref().unwrap();
        assert!(!note.to_lowercase().contains("elsevier"), "{note}");
    }

    // ---------- the acceptance criterion ----------

    /// `action_list` accounts for every missing entry exactly once, and the
    /// human groups plus the deferred buckets sum to the coverage total.
    #[test]
    fn action_list_totals_equal_coverage_totals() {
        let (_dir, db) = open();
        for (paper, kind, status) in [
            ("p-wanted", "fulltext", "needs_ill"),
            ("p-wanted", "si", "pending"),
            ("p-held", "fulltext", "tdm_key_missing"),
            ("p-held", "table", "error"),
            ("p-untouched", "fulltext", "needs_authorisation"),
            ("p-untouched", "si", "rate_limited"),
        ] {
            want(&db, paper, kind, status);
        }
        artefact(&db, "p-untouched", "fulltext_pdf", "vor", "publisher");

        let report = db.coverage_report(None).unwrap();
        let list = report.action_list();
        assert_eq!(list.accounted(), report.missing_total(), "{list:?}");
        assert_eq!(list.human_total() + list.deferred_total(), list.accounted());
        assert!(
            list.human_total() > 0 && list.deferred_total() > 0,
            "the interesting case: entries in both buckets {list:?}"
        );
        assert_eq!(
            list.accounted(),
            report.by_status.values().sum::<usize>(),
            "the per-status table and the grouped view count the same rows"
        );
        assert_eq!(report.missing_total(), 5, "one full text is held");
    }

    /// The partition is exhaustive by construction, not by inspection: every
    /// entry is pushed into exactly one bucket, once per vocabulary row.
    #[test]
    fn every_entry_lands_in_exactly_one_bucket() {
        let (_dir, db) = open();
        for (index, vocab) in ALL_STATUSES.iter().enumerate() {
            add_paper(
                &db,
                &format!("p-status-{index}"),
                Some(&format!("10.1016/j.jneumeth.2005.09.009-{index}")),
            );
            want(&db, &format!("p-status-{index}"), "fulltext", vocab.status);
        }
        let report = db.coverage_report(None).unwrap();
        assert_eq!(report.missing_total(), ALL_STATUSES.len());

        let list = report.action_list();
        let grouped: usize = list.groups.iter().map(ActionGroup::count).sum();
        let deferred: usize = list.deferred.iter().map(|d| d.count).sum();
        assert_eq!(grouped + deferred, ALL_STATUSES.len());
        assert_eq!(grouped, list.human_total());
        let actionable = ALL_STATUSES.iter().filter(|v| v.action.is_some()).count();
        assert_eq!(grouped, actionable);
        assert_eq!(deferred, ALL_STATUSES.len() - actionable);
        assert_eq!(list.accounted(), report.missing_total());
    }

    /// `pending` is never in the human list — ADR-007 §2 marks it "— (picked up
    /// by `acquire`)" — and the report says where it went instead.
    #[test]
    fn a_pending_work_is_not_in_the_human_action_list() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "pending");
        let report = db.coverage_report(None).unwrap();
        let list = report.action_list();
        assert!(list.groups.is_empty(), "{list:?}");
        assert_eq!(list.human_total(), 0, "a human can act on nothing here");
        let deferred = list
            .deferred
            .iter()
            .find(|d| d.status == "pending")
            .expect("pending is reported, just not as a human action");
        assert_eq!(deferred.count, 1);
        assert_eq!(deferred.meaning, "never tried");
        assert!(
            deferred.goes_to.contains("acquire"),
            "and the report says where it went: {}",
            deferred.goes_to
        );
        assert_eq!(list.accounted(), report.missing_total());
    }

    /// The same for the other statuses the ADR leaves out of the human list.
    #[test]
    fn retryable_and_already_fetchable_statuses_are_deferred_too() {
        let (_dir, db) = open();
        for (kind, status) in [
            ("fulltext", "oa_fetchable"),
            ("fulltext_pdf", "tdm_available"),
            ("si", "unavailable"),
            ("table", "rate_limited"),
            ("figure", "error"),
        ] {
            want(&db, "p-wanted", kind, status);
        }
        let list = db.action_list().unwrap();
        assert!(list.groups.is_empty(), "{list:?}");
        assert_eq!(list.deferred_total(), 5);
        assert_eq!(list.accounted(), 5);
        assert_eq!(
            list.deferred.iter().map(|d| d.status).collect::<Vec<_>>(),
            vec![
                "oa_fetchable",
                "tdm_available",
                "unavailable",
                "rate_limited",
                "error"
            ],
            "deferred buckets come out in ADR-007 §2's order"
        );
        assert!(
            list.deferred
                .iter()
                .any(|d| d.status == "unavailable" && d.goes_to.contains("no action"))
        );
        assert!(
            list.deferred
                .iter()
                .any(|d| d.status == "rate_limited" && d.goes_to.contains("next_attempt_at"))
        );
    }

    /// `--kind` narrows the entries and every number computed over them, while
    /// the library-wide scope counters stay whole-library.
    #[test]
    fn filtering_by_kind_renumbers_the_whole_slice() {
        let (_dir, db) = open();
        want(&db, "p-wanted", "fulltext", "needs_ill");
        want(&db, "p-wanted", "si", "pending");
        want(&db, "p-held", "fulltext", "error");

        let all = db.coverage_report(None).unwrap();
        assert_eq!(all.missing_total(), 3);
        assert_eq!(all.by_kind.len(), ALL_WANT_KINDS.len());
        assert_eq!(
            all.by_kind["fulltext"],
            KindTotal {
                wanted: 2,
                missing: 2,
                held: 0
            }
        );

        let filtered = db.coverage_report(Some("si")).unwrap();
        assert_eq!(filtered.missing_total(), 1);
        assert_eq!(filtered.entries[0].kind, "si");
        assert_eq!(
            filtered.by_kind.len(),
            1,
            "only the kind that was asked for"
        );
        assert_eq!(filtered.by_kind["si"].wanted, 1);
        assert_eq!(filtered.by_kind["si"].missing, 1);
        assert_eq!(
            filtered.by_status.len(),
            ALL_STATUSES.len(),
            "every status still appears"
        );
        assert_eq!(filtered.by_status["pending"], 1);
        assert_eq!(filtered.by_status["needs_ill"], 0);
        assert_eq!(
            filtered.works_total, all.works_total,
            "the scope counters are library-wide and are not filtered"
        );
        assert_eq!(filtered.action_list().accounted(), 1);

        // A kind nothing wants still shows up, at zero.
        let empty = db.coverage_report(Some("figure")).unwrap();
        assert_eq!(empty.missing_total(), 0);
        assert_eq!(empty.by_kind["figure"], KindTotal::default());
        assert_eq!(empty.action_list().accounted(), 0);

        let err = db.coverage_report(Some("docx")).unwrap_err();
        assert!(err.to_string().contains("fulltext_pdf"), "{err}");
    }

    /// A version mismatch survives a `--kind` filter as a version mismatch,
    /// rather than being approximated from the unfiltered count.
    #[test]
    fn filtering_recounts_version_mismatches() {
        let (_dir, db) = open();
        artefact(&db, "p-wanted", "fulltext_pdf", "preprint", "arxiv");
        want(&db, "p-wanted", "fulltext", "needs_ill");
        want(&db, "p-wanted", "si", "pending");

        let all = db.coverage_report(None).unwrap();
        assert_eq!(all.held_at_another_version, 1);

        let si = db.coverage_report(Some("si")).unwrap();
        assert_eq!(si.missing_total(), 1);
        assert_eq!(
            si.held_at_another_version, 0,
            "the si gap holds nothing at all"
        );

        let fulltext = db.coverage_report(Some("fulltext")).unwrap();
        assert_eq!(fulltext.held_at_another_version, 1);
    }

    /// Groups come out in ADR-007 §2's status order, so the list reads the way
    /// the vocabulary does rather than by insertion accident.
    #[test]
    fn groups_follow_the_adrs_status_order() {
        let (_dir, db) = open();
        let ids = add_papers(&db, "p-ord-", "1039", 3);
        for (id, status) in ids
            .iter()
            .zip(["needs_ill", "tdm_key_missing", "identity_mismatch"])
        {
            want(&db, id, "fulltext", status);
        }
        let list = db.action_list().unwrap();
        assert_eq!(
            list.groups.iter().map(|g| g.status).collect::<Vec<_>>(),
            vec!["tdm_key_missing", "needs_ill", "identity_mismatch"]
        );
        assert!(list.deferred.is_empty());
        assert_eq!(list.accounted(), 3);
    }

    /// Groups are keyed by (action group, publisher): two works from one
    /// publisher share a group, two publishers do not.
    #[test]
    fn groups_are_keyed_by_action_and_publisher() {
        let (_dir, db) = open();
        let ids = add_papers(&db, "p-g-", "1016", 2);
        for paper in ["p-wanted", &ids[0], &ids[1]] {
            want(&db, paper, "fulltext", "needs_ill");
        }
        let list = db.action_list().unwrap();
        assert_eq!(list.groups.len(), 2, "{list:?}");
        assert_eq!(list.groups[0].publisher.label(), Some("elsevier"));
        assert_eq!(list.groups[0].count(), 2);
        assert_eq!(list.groups[1].publisher.label(), Some("rsc"));
        assert_eq!(list.groups[1].count(), 1);
        assert_eq!(list.accounted(), 3);
    }

    /// One read, one report: the entries the grouped view partitions are the
    /// same objects the coverage projection renders, unrewritten.
    #[test]
    fn the_grouped_view_partitions_the_reported_entries() {
        let (_dir, db) = open();
        let ids = add_papers(&db, "p-part-", "1039", 4);
        for (id, status) in ids
            .iter()
            .zip(["needs_ill", "needs_ill", "pending", "error"])
        {
            want(&db, id, "fulltext", status);
        }
        // A second publisher, so the grouping itself is exercised.
        add_paper(&db, "p-other", Some("10.1016/j.jneumeth.2005.09.009"));
        want(&db, "p-other", "fulltext", "needs_ill");

        let report = db.coverage_report(None).unwrap();
        let list = report.action_list();
        assert_eq!(list.groups.len(), 2, "{list:?}");
        let mut grouped: Vec<MissingEntry> =
            list.groups.iter().flat_map(|g| g.entries.clone()).collect();
        assert_eq!(grouped.len(), list.human_total());
        assert_eq!(grouped.len(), 3, "the two deferred entries stay out of it");
        let mut expected: Vec<MissingEntry> = report
            .entries
            .iter()
            .filter(|e| e.status == "needs_ill")
            .cloned()
            .collect();
        grouped.sort_by_key(|a| a.key());
        expected.sort_by_key(|a| a.key());
        assert_eq!(grouped, expected, "no entry is rewritten on its way in");
        assert_eq!(list.accounted(), report.missing_total());
    }
}
