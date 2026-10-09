//! ADR-007 §1 "Artefact rules": the deterministic artefact id and the
//! row writers for `artefacts` and `acquisition_state`.
//!
//! Both writers are *single-copy* by construction: the column list lives
//! here, not in each caller, so the legacy backfill and the flat-layout
//! importer cannot drift apart on which columns a row has.
//!
//! ## Idempotence
//!
//! The UNIQUE key is `UNIQUE (paper_id, kind, version, locator)` and the
//! `id` is derived from exactly that key (see [`artefact_id`]), so a
//! second run over the same inputs hits the same rows. What differs is
//! *conflict handling*, because the three callers mean different things
//! by "run it again":
//!
//! - [`WriteMode::IgnoreExisting`] — [`crate::sqlite::backfill_legacy_artefacts`].
//!   A legacy row is a fact about what was true when the library was
//!   last written; re-running must never rewrite it.
//! - [`WriteMode::Reconcile`] — `scitadel import-flat`, an explicit
//!   reconciliation of a directory tree against the database (ADR-007
//!   §1 "Artefact rules": manual drop-ins are reconciled only by an
//!   explicit command). If a file's bytes changed between runs the row
//!   follows them; if nothing changed the update is skipped entirely, so
//!   a no-op re-run touches no row and no timestamp.
//! - [`WriteMode::Refetch`] — [`record_download`], a fresh fetch through
//!   one named route. The row follows the route as well as the bytes,
//!   which `Reconcile` deliberately does not: a re-download that
//!   succeeds through arXiv where the first attempt fell through to the
//!   publisher must not keep claiming `publisher`.

use std::collections::HashSet;

use rusqlite::params;
use rusqlite::{Connection, TransactionBehavior};
use scitadel_core::models::{AccessStatus, ArtefactVersion, RouteId};
use scitadel_core::untrusted::UntrustedText;
use sha2::{Digest, Sha256};

use crate::error::DbError;
use crate::sqlite::blobs::hex_digest;

/// A flat/legacy file predates version tracking, so `unknown` rather than
/// a claim of `vor`. ADR-007 §1 "Have" trusts `unknown` on rows from
/// `route IN ('legacy','import_flat')` for exactly this reason.
pub const VERSION_UNKNOWN: &str = "unknown";

/// No machine vouched for these bytes: they were already on disk when we
/// found them, or were handed to us in a directory tree.
pub const ACCESS_BASIS_MANUAL: &str = "manual";

/// `route` sentinel for `scitadel import-flat` (ADR-007 §1 "Legacy
/// data").
pub const ROUTE_IMPORT_FLAT: &str = "import_flat";

/// `locator` for a full-text artefact: nothing identifies it but the
/// work.
pub const FULLTEXT_LOCATOR: &str = "";

/// The `kind`, `format` and MIME of one full-text serialisation.
///
/// One table for every writer, so the legacy backfill and the
/// flat-layout importer cannot disagree about which extension is which
/// kind — and, more importantly, cannot both claim a `.docx` is a full
/// text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FulltextKind {
    pub kind: &'static str,
    pub format: &'static str,
    pub mime: &'static str,
}

/// `(kind, format, mime)` for a full-text file extension, or `None`
/// when that extension is not a full-text kind.
///
/// `xml`/`nxml`/`jats` are included because JATS is a real full-text
/// serialisation and raid consumes it. Anything else gets **no**
/// artefact: `artefacts.kind` is a closed vocabulary of things the "have"
/// derivation understands, so filing a `.docx` as `fulltext_html` would
/// make coverage claim we hold a full text we cannot read.
#[must_use]
pub fn fulltext_kind(ext: &str) -> Option<FulltextKind> {
    match ext {
        "pdf" => Some(FulltextKind {
            kind: "fulltext_pdf",
            format: "pdf",
            mime: "application/pdf",
        }),
        "html" | "htm" | "xhtml" => Some(FulltextKind {
            kind: "fulltext_html",
            format: "html",
            mime: "text/html",
        }),
        "nxml" | "jats" => Some(FulltextKind {
            kind: "fulltext_xml",
            format: "jats",
            mime: "application/xml",
        }),
        "xml" => Some(FulltextKind {
            kind: "fulltext_xml",
            format: "xml",
            mime: "application/xml",
        }),
        _ => None,
    }
}

/// True when the work already holds a full text in any serialisation.
///
/// The first half of ADR-007 §1 "Have" (there is no "have" *status* —
/// it is derived from `artefacts`). Callers that record gaps use it so
/// they never write "wanted" next to something already held.
pub fn has_fulltext_artefact(conn: &Connection, paper_id: &str) -> Result<bool, DbError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM artefacts
         WHERE paper_id = ?1 AND kind IN ('fulltext_pdf','fulltext_html','fulltext_xml')",
        params![paper_id],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

/// What a re-run should do when the UNIQUE key already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// `INSERT OR IGNORE`: leave the existing row exactly as it is.
    IgnoreExisting,
    /// Update the existing row **only** where a value actually
    /// differs, so an unchanged re-run rewrites nothing at all.
    Reconcile,
    /// A fresh fetch through a named route: the row follows the
    /// `route`, the `access_basis` that route establishes, the bytes and
    /// the retrieval time.
    ///
    /// Distinct from [`Self::Reconcile`] in two ways, both load-bearing.
    /// `route` is in the updated set, because a ladder walk can reach the
    /// same work through a different step on a second attempt and the row
    /// must say which one actually served it. And `retrieved_at` is in the
    /// `WHERE` gate, because these bytes *were* fetched again — unlike a
    /// re-scanned file tree, where "unchanged" really does mean unchanged.
    Refetch,
}

/// A `blobs` row to `INSERT OR IGNORE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobWrite {
    pub sha256: String,
    pub bytes: i64,
    pub mime: String,
    pub rel_path: String,
    pub created_at: String,
}

/// The licence a registry stated for the work the fetched document is a copy
/// of — migration 013's four `license_*` columns as one value.
///
/// **Whose licence this is, and what it is a statement about.** A licence
/// offer is a registry's statement about the *work*: Crossref's `license[]`
/// says "this DOI's version of record is CC-BY", not "these bytes are CC-BY".
/// The artefact row it lands on is *one document* of that work, at the version
/// the resolver ranked the candidate at, which is why the offer is threaded
/// from the **chosen candidate** and not from the work: a `vor`-only grant is
/// already filtered out for a preprint candidate before this struct exists
/// (the resolve pass's `counts_for`), so what survives is a grant that covers
/// the version actually stored.
///
/// `url` is not an `Option` because an offer with no URL is not an offer:
/// every constructor in the resolve pass requires one, and a licence we cannot
/// name is not a licence we may record. The other three fields stay `Option`
/// because a registry that states no `content-version`, no `start` or no
/// recognisable source has said nothing about them, and "not stated" is what
/// `NULL` means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenceWrite {
    /// The grant's URL, verbatim as the registry spelled it.
    pub url: String,
    /// `content-version`, when the registry declared one.
    pub content_version: Option<String>,
    /// `start`, when the registry declared one.
    pub start: Option<String>,
    /// Which registry stated it, in that registry's own label — recorded
    /// rather than derived, because "Crossref said so" and "Unpaywall said so"
    /// are different provenance and a mirror that collapsed them could not be
    /// checked against the registry that made the claim.
    pub source: String,
}

/// One `artefacts` row. Its `blob`, when present, is written first —
/// `artefacts.sha256` references it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtefactWrite {
    /// Derived from the UNIQUE key. Leave it empty and [`write_artefacts`]
    /// derives it, so a caller cannot pair a row with a stale id.
    pub id: String,
    pub paper_id: String,
    pub kind: String,
    pub version: String,
    pub locator: String,
    /// `None` only for a figure reference with no bytes (ADR-007 §1
    /// "Artefact rules") and for a file we could not read.
    pub sha256: Option<String>,
    pub format: Option<String>,
    pub access_status: String,
    pub route: String,
    pub access_basis: String,
    pub label: Option<String>,
    pub caption: Option<String>,
    pub source_url: Option<String>,
    /// A publisher label we can name, or `None` when the DOI's registrant
    /// prefix is not in the table. Never a guess (#261).
    pub publisher: Option<String>,
    /// Why `publisher` is what it is — see migration 014. `Some` only
    /// when we could *not* name one, and then only ever the verbatim
    /// `RouteVerdict::PublisherUnknown` note, never a claim about TDM
    /// route availability.
    pub publisher_note: Option<String>,
    /// The licence the metadata pass attributed to the bytes, as
    /// [`LicenceWrite`]'s four fields. All four `None` when the chosen
    /// candidate's source named no licence — which is *not* "there is no
    /// licence", it is "none was recorded", and `access_basis` remains the
    /// only licence-adjacent claim the row makes.
    pub license_url: Option<String>,
    pub license_content_version: Option<String>,
    pub license_start: Option<String>,
    pub license_source: Option<String>,
    pub imported_from: Option<String>,
    pub retrieved_at: String,
    /// The path is recorded but we hold no usable bytes for it.
    pub missing_on_disk: bool,
    pub blob: Option<BlobWrite>,
}

/// One `acquisition_state` row: what we want and have not got. There is
/// no "have" status, so this table never asserts possession — it only
/// records the gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateWrite {
    pub paper_id: String,
    pub kind: String,
    pub locator: String,
    pub wanted_version: String,
    pub status: String,
    pub reason: Option<String>,
    /// A publisher we can name, or `None` when the DOI's registrant prefix is
    /// not in [`scitadel_core::publisher`]'s table. Never a guess (#261) —
    /// and `None` is the *correct* answer for a writer that did not classify
    /// anything, which is different from "there is no publisher".
    ///
    /// Both existing writers pass `None` deliberately: the flat importer
    /// names no publisher, and the ladder's gap is grouped by the DOI
    /// registry's answer at report time ([`crate::sqlite::coverage`]) rather
    /// than from this column. `acquire` is the writer that fills it, because
    /// it is the writer that has already classified the work's DOI.
    pub publisher: Option<String>,
    pub hint_url: Option<String>,
    /// Where a human should put the file to close this gap.
    pub drop_path: Option<String>,
    /// When a later run should pick this gap up again, RFC 3339 UTC
    /// (ADR-007 §2: `rate_limited` means "retry at `next_attempt_at`").
    ///
    /// `None` means *no retry is scheduled* — the gap is due now, or a human
    /// owns it. This is the column `--resume` reads, so a row whose only
    /// change is this one must still update: it is in the `WHERE` guard of
    /// [`write_acquisition_states`].
    pub next_attempt_at: Option<String>,
    pub updated_at: String,
}

/// Deterministic artefact id: the first 16 bytes of SHA-256 over a
/// length-framed `paper_id | kind | version | locator`, rendered as 32
/// lowercase hex — the same id shape the workspace already uses for
/// papers, questions and searches.
///
/// Deterministic rather than a fresh `uuid::Uuid::new_v4()` because
/// idempotence should not depend on the UNIQUE index alone: the same
/// input must derive the same id on every run and on every machine, so a
/// re-run after an interrupted transaction cannot leave a row whose id
/// differs from the one a later run would compute. The domain prefix
/// keeps these digests from ever colliding with another sha256-derived
/// hex id in the database, and the length framing keeps `("ab", "c")`
/// from colliding with `("a", "bc")`.
#[must_use]
pub fn artefact_id(paper_id: &str, kind: &str, version: &str, locator: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"scitadel/artefact-id/v1");
    for part in [paper_id, kind, version, locator] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex_digest(&hasher.finalize()[..16])
}

/// Write the given `artefacts` rows (and their `blobs`) in one short
/// transaction, returning how many rows were written.
///
/// `blobs` goes first: `artefacts.sha256` is a foreign key into it, and
/// `PRAGMA foreign_keys` is on.
pub fn write_artefacts(
    conn: &mut Connection,
    rows: &[ArtefactWrite],
    mode: WriteMode,
) -> Result<usize, DbError> {
    if rows.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let written = write_artefacts_in(&tx, rows, mode)?;
    tx.commit()?;
    Ok(written)
}

/// [`write_artefacts`], on a connection or transaction the **caller** owns.
///
/// This is the half that can join somebody else's transaction, which
/// [`record_download`] needs: the artefact row and the legacy
/// `papers` columns are one atomic fact, so they cannot each open their
/// own. `&Connection` rather than a trait object because
/// `rusqlite::Transaction` derefs to `Connection`, so the same call
/// serves both and there is no second code path to keep in step.
pub fn write_artefacts_in(
    conn: &Connection,
    rows: &[ArtefactWrite],
    mode: WriteMode,
) -> Result<usize, DbError> {
    let statement = artefacts_insert(mode);
    for row in rows {
        if let Some(blob) = &row.blob {
            conn.execute(
                "INSERT OR IGNORE INTO blobs (sha256, bytes, mime, rel_path, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    blob.sha256,
                    blob.bytes,
                    blob.mime,
                    blob.rel_path,
                    blob.created_at
                ],
            )?;
        }
        conn.execute(
            &statement,
            params![
                if row.id.is_empty() {
                    artefact_id(&row.paper_id, &row.kind, &row.version, &row.locator)
                } else {
                    row.id.clone()
                },
                row.paper_id,
                row.kind,
                row.version,
                row.locator,
                row.sha256,
                row.format,
                row.access_status,
                row.route,
                row.access_basis,
                row.label,
                row.caption,
                row.source_url,
                row.publisher,
                row.publisher_note,
                row.license_url,
                row.license_content_version,
                row.license_start,
                row.license_source,
                row.imported_from,
                row.retrieved_at,
                i64::from(row.missing_on_disk),
            ],
        )?;
    }
    Ok(rows.len())
}

/// The `artefacts` INSERT for a [`WriteMode`].
///
/// `Reconcile` updates only where a value genuinely differs, which is
/// what makes a no-op re-run touch no row — including `retrieved_at`,
/// which would otherwise be a fresh timestamp on every invocation and
/// make "changed nothing" untestable. `sha256`, `format` and
/// `missing_on_disk` move together, so a replaced file is recorded as
/// the new bytes rather than kept beside its old hash.
///
/// `Refetch` moves `route` and `access_basis` too, because these bytes
/// arrived through a named route that established a basis, and a row
/// left claiming the previous route's answer would misreport where the
/// bytes came from and what may be done with them. The four `license_*`
/// columns move for the same reason and by the same rule: they are the
/// resolve pass's answer for *this* fetch, so a re-download that reached
/// the work through a source that named no licence must clear the grant
/// the previous source named rather than carry it beside bytes it was
/// never said about. `Reconcile` deliberately leaves them alone, like
/// `label` and `caption`: a re-scanned file tree states nothing about a
/// licence, so it has nothing to move.
///
/// It also clears `imported_from`, which is the one field a fetch must
/// take away rather than set: a re-download that collides with a
/// backfilled or flat-imported row (same paper, kind, version and
/// locator) would otherwise keep citing the *old* file's absolute path
/// beside bytes fetched from somewhere else entirely. `label` and
/// `caption` are deliberately left alone — they belong to the
/// `si`/`table`/`figure` slots, which no fetch writes today, and nulling
/// them would destroy a caption the fetch knows nothing about.
fn artefacts_insert(mode: WriteMode) -> String {
    let values = "(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)";
    let columns = "(id, paper_id, kind, version, locator, sha256, format, access_status,
                   route, access_basis, label, caption, source_url, publisher,
                   publisher_note, license_url, license_content_version, license_start,
                   license_source, imported_from, retrieved_at, missing_on_disk)";
    match mode {
        WriteMode::IgnoreExisting => {
            format!("INSERT OR IGNORE INTO artefacts {columns} VALUES {values}")
        }
        WriteMode::Reconcile => format!(
            "INSERT INTO artefacts {columns} VALUES {values}
             ON CONFLICT (paper_id, kind, version, locator) DO UPDATE SET
               sha256 = excluded.sha256,
               format = excluded.format,
               access_status = excluded.access_status,
               label = excluded.label,
               caption = excluded.caption,
               imported_from = excluded.imported_from,
               retrieved_at = excluded.retrieved_at,
               missing_on_disk = excluded.missing_on_disk
             WHERE artefacts.sha256 IS NOT excluded.sha256
                OR artefacts.format IS NOT excluded.format
                OR artefacts.access_status IS NOT excluded.access_status
                OR artefacts.label IS NOT excluded.label
                OR artefacts.caption IS NOT excluded.caption
                OR artefacts.imported_from IS NOT excluded.imported_from
                OR artefacts.retrieved_at IS NOT excluded.retrieved_at
                OR artefacts.missing_on_disk IS NOT excluded.missing_on_disk"
        ),
        WriteMode::Refetch => format!(
            "INSERT INTO artefacts {columns} VALUES {values}
             ON CONFLICT (paper_id, kind, version, locator) DO UPDATE SET
               sha256 = excluded.sha256,
               format = excluded.format,
               access_status = excluded.access_status,
               route = excluded.route,
               access_basis = excluded.access_basis,
               source_url = excluded.source_url,
               publisher = excluded.publisher,
               publisher_note = excluded.publisher_note,
               license_url = excluded.license_url,
               license_content_version = excluded.license_content_version,
               license_start = excluded.license_start,
               license_source = excluded.license_source,
               imported_from = excluded.imported_from,
               retrieved_at = excluded.retrieved_at
             WHERE artefacts.sha256 IS NOT excluded.sha256
                OR artefacts.format IS NOT excluded.format
                OR artefacts.access_status IS NOT excluded.access_status
                OR artefacts.route IS NOT excluded.route
                OR artefacts.access_basis IS NOT excluded.access_basis
                OR artefacts.source_url IS NOT excluded.source_url
                OR artefacts.publisher IS NOT excluded.publisher
                OR artefacts.publisher_note IS NOT excluded.publisher_note
                OR artefacts.license_url IS NOT excluded.license_url
                OR artefacts.license_content_version IS NOT excluded.license_content_version
                OR artefacts.license_start IS NOT excluded.license_start
                OR artefacts.license_source IS NOT excluded.license_source
                OR artefacts.imported_from IS NOT excluded.imported_from
                OR artefacts.retrieved_at IS NOT excluded.retrieved_at"
        ),
    }
}

/// One completed download, ready to be recorded against a work.
///
/// Everything the caller *decided* is here; everything the schema *derives* is
/// not. `kind`, `format` and `mime` come from `ext` through [`fulltext_kind`],
/// and `access_basis` comes from [`RouteId`] — so no caller can pair a route
/// with another route's licence answer, which is the drift #261 exists to stop.
///
/// `version` is the one field the caller may also set, through
/// [`Self::version`], and the asymmetry is deliberate. See
/// [`record_download`] for the precedence and for why the licence has no such
/// field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadWrite {
    /// The work these bytes belong to. Must already be a `papers` row.
    pub paper_id: String,
    /// Which ladder step served them (ADR-007 §3).
    pub route: RouteId,
    /// The version of the work these bytes **are**, when the caller established
    /// one; `None` to fall back to [`RouteId::artefact_version`].
    ///
    /// `Some(ArtefactVersion::Unknown)` and `None` are *different* inputs and
    /// they are not interchangeable: `None` says "I did not look, ask the
    /// route", `Some(Unknown)` says "I looked and nothing stated a version". The
    /// second is stronger and the resolver is the only producer of it, because
    /// the resolver is the only thing that looked.
    ///
    /// Only a caller that compared the registries may set this. A caller that
    /// invented a version here would be #261's overclaim with the licence
    /// guard removed, so there is deliberately nowhere for a *guess* to go: the
    /// field takes a closed vocabulary and nothing else.
    pub version: Option<ArtefactVersion>,
    /// File extension of what we stored: `pdf`, `html`, `xml`…
    pub ext: String,
    pub access_status: AccessStatus,
    /// The URL the bytes came from, after redirects.
    pub source_url: Option<String>,
    /// A publisher we can name, or `None`.
    pub publisher: Option<String>,
    /// Why `publisher` is empty — see [`ArtefactWrite::publisher_note`].
    pub publisher_note: Option<String>,
    /// The licence the metadata pass attributed to **this** fetch, or `None`
    /// when the source that served the bytes named no licence. See
    /// [`LicenceWrite`] for whose statement this is and why it travels with the
    /// chosen candidate.
    ///
    /// There is no route-level fallback, deliberately: `RouteId::access_basis`
    /// already records *why access is lawful* and no route states a reuse
    /// grant. Deriving a licence from the route would be #261's overclaim with
    /// a URL attached.
    pub licence: Option<LicenceWrite>,
    /// RFC 3339 UTC: when these bytes were fetched.
    pub retrieved_at: String,
    /// The content-addressed copy. `None` means the caller could not put
    /// the bytes in the store, which is a reason to refuse the whole
    /// write rather than to record a row pointing at nothing.
    pub blob: Option<BlobWrite>,
}

/// Record one completed download: the `blobs` row and the `artefacts` row, in a
/// **single** `BEGIN IMMEDIATE`.
///
/// One transaction, because the two rows are one fact — a `blobs` row no artefact
/// references is garbage `gc` has to reason about, and an artefact pointing at a
/// blob that was never written is a file we claim to hold and cannot open.
///
/// # The legacy columns are not written here (#253 S2e)
///
/// Through S1 this also filled `papers.local_path`, `download_status` and
/// `last_attempt_at`, so that the TUI state column, `find_cached_file` and
/// `read_paper` kept working while `artefacts` was still the newer shape. All
/// three of those readers now go through ADR-007 §1's derivation
/// (`sqlite::coverage`), so the second write was the last thing keeping two
/// answers to "do we have this paper" alive, and it was the one that went stale
/// first: a re-fetch through a different route moved the artefact and left
/// `local_path` pointing at the previous file.
///
/// The `papers` row is still **required** to exist — `artefacts.paper_id` is a
/// foreign key into it, and that insert is what refuses otherwise. What is gone is
/// the `UPDATE` of columns nothing reads.
///
/// `INSERT OR IGNORE` on `blobs` is deliberate even though the artefact
/// row is an upsert: the blob store is content-addressed, so an existing
/// row is by definition the same bytes, and rewriting its `created_at`
/// would break the backfill's "a second run changes nothing" guarantee.
/// The `artefacts` row itself uses [`WriteMode::Refetch`], because a
/// re-download *did* happen: it must move `route`, `sha256` and
/// `retrieved_at`.
///
/// # Errors
///
/// `DbError` if anything about the write fails. Three refusals in
/// particular:
///
/// - a route with no `artefact_version()` or no `access_basis()`. Both
///   columns are `NOT NULL`, and "this route establishes no basis" must
///   not become a guess — a hard error keeps the caller from inventing
///   one;
/// - an extension outside the `kind` vocabulary, for the same reason
///   `fulltext_kind` exists: filing a `.docx` as `fulltext_html` would
///   make coverage claim a full text we cannot read;
/// - a `paper_id` with no `papers` row. With `PRAGMA foreign_keys` on —
///   which every connection out of [`crate::sqlite::Database`] has — the
///   artefacts insert is what refuses, and the error names the foreign key.
///
/// The caller propagates all of them: **a download that cannot be
/// recorded is not a completed download.**
///
/// ## The want-list is retracted in the same transaction (#260)
///
/// ADR-007 §1 states the invariant this transaction upholds:
/// `acquisition_state` records only what we want and have not got. So when a
/// download closes a gap, the gap goes — in this transaction, or a reader
/// between the two writes would see a work that is both held and wanted, and
/// `coverage` would report a full text we are already holding.
///
/// The delete is scoped twice over, because two writers share this table and
/// only one of them writes a gap with no `drop_path`:
///
/// - `kind = 'fulltext'` and `locator = ''` — the full-text want, whatever
///   serialisation the fetch turned out to be. SI, table and figure wants are
///   not closed by a full-text download.
/// - `drop_path IS NULL` — the acquisition ladder's signature. The flat
///   importer's gap rows always carry the path a human should drop the file
///   at, and are retracted by
///   [`delete_acquisition_states_at`] with that exact path; this statement
///   must not take them back on the ladder's behalf.
pub fn record_download(conn: &mut Connection, write: &DownloadWrite) -> Result<(), DbError> {
    let kind = fulltext_kind(&write.ext).ok_or_else(|| {
        DbError::Migration(format!(
            "{:?} is not a full-text kind, so a download of it cannot be recorded as an artefact",
            write.ext
        ))
    })?;
    // `version` and `access_basis` are both NOT NULL, and both are *answers*
    // rather than defaults.
    //
    // ## The version precedence, and why it is the caller's
    //
    // `write.version` wins when the caller set it, and
    // `RouteId::artefact_version()` is the fallback. The caller is the resolve-
    // then-rank pass, which compared the registries that registered this DOI and
    // read a version word off the location it chose; the route knows what kind of
    // server served the bytes and nothing about what those bytes turned out to
    // be. `RouteId::artefact_version`'s own docs say the same.
    //
    // **This is load-bearing, not a convenience.** Filing the route's fallback
    // `unknown` for a version-of-record fetch leaves `version_satisfies` false
    // for a `vor` want — `UNTRACKED_VERSION_ROUTES` is `legacy`/`import_flat` —
    // so `coverage` reports the want unsatisfied, the queue re-queues the work,
    // and the next run ranks the same candidate and fetches the same document
    // again, for ever. That loop is what
    // `a_version_of_record_fetch_and_recorded_satisfies_a_vor_want` exists to
    // close.
    //
    // ## Why the licence has no such field
    //
    // `access_basis` is derived from the route and **cannot** be overridden,
    // because there is no second producer for it: nothing in the resolve pass
    // establishes what we may reuse beyond "a grant the registries published, in
    // force for this version", and that is recorded on the licence columns rather
    // than invented here. #261's rule — no caller may pair a route with another
    // route's licence answer — is therefore still enforced by there being
    // nowhere to put one, and only the axis the resolver is *entitled* to answer
    // gained a field.
    //
    // ## Why the licence columns are not derived from the route either
    //
    // A licence offer is a registry's statement about the *work* the chosen
    // candidate serves, filtered to the version that candidate was ranked at
    // before it reaches this function. So the one thing this function does with
    // it is copy it: four columns, no derivation, no default. `None` stays
    // `None` — "no licence was recorded" and "there is no licence" are different
    // facts and only one of them is in the row.
    let version = write
        .version
        .or_else(|| write.route.artefact_version())
        .ok_or_else(|| {
            DbError::Migration(format!(
                "route {} establishes no artefact version — it is a pseudo-route that never fetches",
                write.route.label()
            ))
        })?;
    let access_basis = write.route.access_basis().ok_or_else(|| {
        DbError::Migration(format!(
            "route {} establishes no access basis; access_basis is NOT NULL and \
             \"we did not look\" must not be recorded as a guess",
            write.route.label()
        ))
    })?;
    let blob = write.blob.clone().ok_or_else(|| {
        DbError::Migration(format!(
            "no blob recorded for the {} download of {}",
            write.route.label(),
            write.paper_id
        ))
    })?;

    let row = ArtefactWrite {
        // Derived, not generated: see [`artefact_id`].
        id: String::new(),
        paper_id: write.paper_id.clone(),
        kind: kind.kind.to_string(),
        version: version.label().to_string(),
        locator: FULLTEXT_LOCATOR.to_string(),
        sha256: Some(blob.sha256.clone()),
        format: Some(kind.format.to_string()),
        access_status: write.access_status.label().to_string(),
        route: write.route.label().to_string(),
        access_basis: access_basis.to_string(),
        label: None,
        caption: None,
        source_url: write.source_url.clone(),
        publisher: write.publisher.clone(),
        publisher_note: write.publisher_note.clone(),
        // Copied, not derived: see the docs above on why a licence has no
        // route-level fallback and why "absent" and "no licence" differ.
        license_url: write.licence.as_ref().map(|licence| licence.url.clone()),
        license_content_version: write
            .licence
            .as_ref()
            .and_then(|licence| licence.content_version.clone()),
        license_start: write
            .licence
            .as_ref()
            .and_then(|licence| licence.start.clone()),
        license_source: write.licence.as_ref().map(|licence| licence.source.clone()),
        imported_from: None,
        retrieved_at: write.retrieved_at.clone(),
        missing_on_disk: false,
        blob: Some(blob),
    };

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // blobs, then artefacts: artefacts.sha256 is a foreign key into blobs.
    write_artefacts_in(&tx, std::slice::from_ref(&row), WriteMode::Refetch)?;
    // The full-text want this download just satisfied, retracted in the same
    // transaction — see the docs above for the two clauses that keep the
    // statement off the flat importer's rows.
    tx.execute(
        "DELETE FROM acquisition_state
         WHERE paper_id = ?1 AND kind = 'fulltext' AND locator = '' AND drop_path IS NULL",
        params![write.paper_id],
    )?;
    tx.commit()?;
    Ok(())
}

/// Upsert `acquisition_state` rows, returning how many were written.
///
/// Like [`write_artefacts`] in `WriteMode::Reconcile`, an identical
/// re-run updates nothing — a gap row's `updated_at` should say when the
/// gap last *changed*, not when someone looked at it again.
///
/// ## `publisher` and `next_attempt_at` are in the `WHERE` guard too
///
/// Both columns are in the updated set *and* in the guard, which is the
/// only combination that lets "the status did not change but the retry time
/// did" be a real update. Leaving them out of the guard — as
/// [`write_artefacts`] deliberately does for a few `artefacts` columns —
/// would make a row whose only change is `next_attempt_at` a silent no-op:
/// `INSERT ... ON CONFLICT DO UPDATE ... WHERE <false>` updates nothing, and
/// `--resume` would then keep re-fetching a work it had already deferred,
/// forever. The whole point of the column is that it *changes independently*
/// of the status, so it is in the guard.
pub fn write_acquisition_states(
    conn: &mut Connection,
    rows: &[StateWrite],
) -> Result<usize, DbError> {
    if rows.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    write_acquisition_states_in(&tx, rows)?;
    tx.commit()?;
    Ok(rows.len())
}

/// [`write_acquisition_states`], on a connection or transaction the **caller**
/// owns.
///
/// The half that can join somebody else's transaction, which
/// `override_identity` needs: it writes the identity override and returns a
/// mismatched gap to the `acquire` queue as one atomic fact, so a reader
/// between the two writes could never see a work whose identity a person has
/// settled and whose want row still says the machine blocked it. `rusqlite`'s
/// `Transaction` deliberately does not implement `DerefMut`, so this cannot be
/// reached by handing the transaction to [`write_acquisition_states`].
pub fn write_acquisition_states_in(
    conn: &Connection,
    rows: &[StateWrite],
) -> Result<usize, DbError> {
    for row in rows {
        conn.execute(
            "INSERT INTO acquisition_state
                 (paper_id, kind, locator, wanted_version, status, reason, publisher,
                  hint_url, drop_path, next_attempt_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (paper_id, kind, locator) DO UPDATE SET
               wanted_version = excluded.wanted_version,
               status = excluded.status,
               reason = excluded.reason,
               publisher = excluded.publisher,
               hint_url = excluded.hint_url,
               drop_path = excluded.drop_path,
               next_attempt_at = excluded.next_attempt_at,
               updated_at = excluded.updated_at
             WHERE acquisition_state.wanted_version IS NOT excluded.wanted_version
                OR acquisition_state.status IS NOT excluded.status
                OR acquisition_state.reason IS NOT excluded.reason
                OR acquisition_state.publisher IS NOT excluded.publisher
                OR acquisition_state.hint_url IS NOT excluded.hint_url
                OR acquisition_state.drop_path IS NOT excluded.drop_path
                OR acquisition_state.next_attempt_at IS NOT excluded.next_attempt_at",
            params![
                row.paper_id,
                row.kind,
                row.locator,
                row.wanted_version,
                row.status,
                row.reason,
                row.publisher,
                row.hint_url,
                row.drop_path,
                row.next_attempt_at,
                row.updated_at,
            ],
        )?;
    }
    Ok(rows.len())
}

/// One `acquisition_state` row as stored — the reader's mirror of
/// [`StateWrite`].
///
/// A whole row rather than a projected subset because every caller that reads
/// one has to write it back: `acquire` reads the status a *walk* established
/// and re-writes it with `next_attempt_at` set, which means reading a subset
/// would mean inventing the values for the columns it did not read. Reuse is
/// the point — a reader that reconstructed a `StateWrite` from parts is how
/// `publisher` or `drop_path` gets dropped on the way through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRow {
    pub paper_id: String,
    pub kind: String,
    pub locator: String,
    pub wanted_version: String,
    pub status: String,
    pub reason: Option<String>,
    pub publisher: Option<String>,
    pub hint_url: Option<String>,
    pub drop_path: Option<String>,
    pub next_attempt_at: Option<String>,
    pub updated_at: String,
}

impl StateRow {
    /// The [`StateWrite`] that reproduces this row, so a read-modify-write
    /// cannot lose a column.
    #[must_use]
    pub fn to_write(&self) -> StateWrite {
        StateWrite {
            paper_id: self.paper_id.clone(),
            kind: self.kind.clone(),
            locator: self.locator.clone(),
            wanted_version: self.wanted_version.clone(),
            status: self.status.clone(),
            reason: self.reason.clone(),
            publisher: self.publisher.clone(),
            hint_url: self.hint_url.clone(),
            drop_path: self.drop_path.clone(),
            next_attempt_at: self.next_attempt_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

/// Read the gap row for `(paper_id, kind, locator)`, if there is one.
///
/// The primary key is exactly those three columns, so this is the whole
/// identity of a want and a caller cannot accidentally read a neighbour's row.
/// `None` means no want is recorded — which is *not* the same as "held": only
/// the ADR-007 §1 derivation in [`crate::sqlite::coverage`] can say a want is
/// satisfied, and a caller that needs that must ask it.
pub fn read_acquisition_state(
    conn: &Connection,
    paper_id: &str,
    kind: &str,
    locator: &str,
) -> Result<Option<StateRow>, DbError> {
    let row = conn.query_row(
        "SELECT paper_id, kind, locator, wanted_version, status, reason, publisher,
                hint_url, drop_path, next_attempt_at, updated_at
         FROM acquisition_state
         WHERE paper_id = ?1 AND kind = ?2 AND locator = ?3",
        params![paper_id, kind, locator],
        |r| {
            Ok(StateRow {
                paper_id: r.get(0)?,
                kind: r.get(1)?,
                locator: r.get(2)?,
                wanted_version: r.get(3)?,
                status: r.get(4)?,
                reason: r.get(5)?,
                publisher: r.get(6)?,
                hint_url: r.get(7)?,
                drop_path: r.get(8)?,
                next_attempt_at: r.get(9)?,
                updated_at: r.get(10)?,
            })
        },
    );
    match row {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Delete the gap rows for `paper_id` recorded at `drop_path`.
///
/// The importer calls this for a gap it previously recorded and has now
/// closed. Scoping by `drop_path` is what makes the retraction honest:
/// we only take back a row naming the exact file we just imported, so a
/// pending row written by the acquisition ladder — which has no
/// `drop_path`, or a different one — is never silently deleted. Leaving
/// a satisfied gap row behind would be worse than deleting nothing: the
/// "have" derivation would report the full text as both held and wanted.
pub fn delete_acquisition_states_at(
    conn: &Connection,
    paper_id: &str,
    drop_path: &str,
) -> Result<usize, DbError> {
    Ok(conn.execute(
        "DELETE FROM acquisition_state
         WHERE paper_id = ?1 AND drop_path IS NOT NULL AND drop_path = ?2",
        params![paper_id, drop_path],
    )?)
}

/// One `artefacts` row **as stored**, joined with its blob — the reader's
/// mirror of [`ArtefactWrite`].
///
/// A whole row rather than a projected subset for the same reason [`StateRow`] is
/// one: every caller that reads an artefact has to be able to describe it
/// faithfully somewhere else, and a subset invites a caller to invent the values
/// for the columns it did not read. The manifest mirror is the caller today
/// (`scitadel_adapters::manifest`), and a provenance document that quietly omits
/// `license_url` because the reader did not select it is worse than no document.
///
/// The licence columns are here because ADR-007 §1 says `license_url` is
/// "licence + provenance" and the consumer decides redistribution from it; a
/// mirror that left it out would make the mirror unusable for the one purpose it
/// exists.
///
/// `bytes` and `blob_rel_path` come from a `LEFT JOIN` on `blobs`, so a figure
/// reference with no bytes (`sha256 IS NULL`) and an artefact whose blob row is
/// absent both read as `None` rather than dropping the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtefactRow {
    pub id: String,
    pub paper_id: String,
    pub kind: String,
    pub version: String,
    pub locator: String,
    pub sha256: Option<String>,
    pub format: Option<String>,
    pub access_status: String,
    pub route: String,
    pub access_basis: String,
    pub label: Option<String>,
    pub caption: Option<String>,
    pub source_url: Option<String>,
    pub publisher: Option<String>,
    pub publisher_note: Option<String>,
    pub imported_from: Option<String>,
    pub retrieved_at: String,
    pub missing_on_disk: bool,
    pub license_url: Option<String>,
    pub license_content_version: Option<String>,
    pub license_start: Option<String>,
    pub license_source: Option<String>,
    /// The blob's size, or `None` when there are no bytes.
    pub bytes: Option<i64>,
    /// The blob's **root-relative** path (ADR-007 §1 "Storage"), or `None`.
    pub blob_rel_path: Option<String>,
}

impl ArtefactRow {
    /// This artefact's human label, as untrusted display text (ADR-007 §1's
    /// untrusted-content envelope).
    ///
    /// **Always [`scitadel_core::untrusted::Provenance::PublisherSupplied`]**, and
    /// that is a statement about the whole column rather than about one writer: a
    /// `label` is a *slot name*, and every slot name in this workspace came out of
    /// a file somebody else chose. `import_flat` takes it from a filename
    /// (`<slug>_SI_<name>`), `scan` and `attach` from the dropped file's own stem,
    /// and the flat layout's `tables.json` for a table. scitadel writes no label of
    /// its own, so marking one `Ours` would be a lie the type could not keep.
    ///
    /// The raw string is behind [`UntrustedText::as_str`], which is what the
    /// manifest mirror serialises: the mirror records what the publisher said, and
    /// that fidelity is a separate requirement from being safe to display.
    #[must_use]
    pub fn label_text(&self) -> Option<UntrustedText> {
        self.label.as_deref().map(UntrustedText::publisher_supplied)
    }

    /// This artefact's caption, as untrusted display text.
    ///
    /// A caption is a publisher's or a depositor's own sentence about a figure or a
    /// table, verbatim: `meta.json` for a figure, `tables.json` for a table, a
    /// `<id>.caption.txt` sidecar otherwise. Same provenance as
    /// [`Self::label_text`], and the same reason.
    #[must_use]
    pub fn caption_text(&self) -> Option<UntrustedText> {
        self.caption
            .as_deref()
            .map(UntrustedText::publisher_supplied)
    }
}

/// Every artefact row for `paper_id`, in `(kind, locator)` order.
///
/// The order is `coverage.rs`'s, so a projection of this list and a projection of
/// the report cannot disagree about which artefact comes first.
pub fn read_artefacts_for_paper(
    conn: &Connection,
    paper_id: &str,
) -> Result<Vec<ArtefactRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.paper_id, a.kind, a.version, a.locator, a.sha256, a.format,
                a.access_status, a.route, a.access_basis, a.label, a.caption, a.source_url,
                a.publisher, a.publisher_note, a.imported_from, a.retrieved_at,
                a.missing_on_disk, a.license_url, a.license_content_version, a.license_start,
                a.license_source, b.bytes, b.rel_path
           FROM artefacts a
           LEFT JOIN blobs b ON b.sha256 = a.sha256
          WHERE a.paper_id = ?1
          ORDER BY a.kind, a.locator",
    )?;
    let rows = stmt.query_map(params![paper_id], |row| {
        Ok(ArtefactRow {
            id: row.get(0)?,
            paper_id: row.get(1)?,
            kind: row.get(2)?,
            version: row.get(3)?,
            locator: row.get(4)?,
            sha256: row.get(5)?,
            format: row.get(6)?,
            access_status: row.get(7)?,
            route: row.get(8)?,
            access_basis: row.get(9)?,
            label: row.get(10)?,
            caption: row.get(11)?,
            source_url: row.get(12)?,
            publisher: row.get(13)?,
            publisher_note: row.get(14)?,
            imported_from: row.get(15)?,
            retrieved_at: row.get(16)?,
            missing_on_disk: row.get(17)?,
            license_url: row.get(18)?,
            license_content_version: row.get(19)?,
            license_start: row.get(20)?,
            license_source: row.get(21)?,
            bytes: row.get(22)?,
            blob_rel_path: row.get(23)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Delete the **drop-path** gap rows for `(paper_id, kind, locator)` — the gaps
/// that a file arriving closes.
///
/// The companion to [`delete_acquisition_states_at`], and deliberately a different
/// scope: that one matches an exact `drop_path` string, which is what the
/// flat-layout importer needs (it knows which path it recorded). This one matches
/// the **want**, not the path, because a drop-in is recorded from a directory
/// that has been resolved (`/private/var/…` on macOS, wherever a symlink led) and
/// a string comparison against an unresolved path retracts nothing at all — a
/// silent no-op that leaves `coverage` reporting a file we are already holding.
///
/// Two clauses keep it off rows that are not ours:
///
/// - `kind` and `locator` name exactly one want, so a `table` drop-in never
///   closes the full-text gap beside it;
/// - `drop_path IS NOT NULL` is the drop-in signature — the acquisition ladder's
///   `pending` row has no `drop_path` and is closed by a **fetch**, through
///   `record_download`'s own retraction, not by a person dropping a file.
pub fn delete_drop_path_states(
    conn: &Connection,
    paper_id: &str,
    kind: &str,
    locator: &str,
) -> Result<usize, DbError> {
    Ok(conn.execute(
        "DELETE FROM acquisition_state
          WHERE paper_id = ?1 AND kind = ?2 AND locator = ?3 AND drop_path IS NOT NULL",
        params![paper_id, kind, locator],
    )?)
}

/// Distinct blob digests among `rows`, for a log line ("how many new
/// blobs did this cost?").
#[must_use]
pub fn distinct_blob_count<'a, I>(rows: I) -> usize
where
    I: IntoIterator<Item = &'a ArtefactWrite>,
{
    rows.into_iter()
        .filter_map(|r| r.blob.as_ref().map(|b| b.sha256.as_str()))
        .collect::<HashSet<_>>()
        .len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::{Database, ROUTE_LEGACY};

    /// A fixed stamp, so an assertion is about the value rather than the clock.
    const NOW: &str = "2026-01-01T00:00:00+00:00";

    /// A file-backed library with one `papers` row, for a real `record_download`.
    fn library_with(paper_id: &str) -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("scitadel.db")).expect("open");
        db.migrate().expect("migrate");
        db.conn()
            .expect("conn")
            .execute(
                "INSERT INTO papers (id, title, authors, created_at, updated_at)
                 VALUES (?1, 'A work', '[]', ?2, ?2)",
                [paper_id, NOW],
            )
            .expect("paper row");
        // `TempDir` must outlive the connection: the pool opens lazily, so
        // dropping the directory would let a later `conn()` create an empty one.
        (dir, db)
    }

    /// **The version that reaches `artefacts.version`**, over a real library and
    /// the real `record_download`, for all four ways a caller can pair a route
    /// with a claim.
    ///
    /// This is the join [`RouteId::artefact_version`]'s docs point at and cannot
    /// test, because `scitadel-db` depends on `scitadel-core` and the precedence
    /// lives inside this function. The four rows are the whole contract:
    ///
    /// | caller `version` | route | column | why |
    /// |---|---|---|---|
    /// | `Some(vor)` | `Arxiv` | `vor` | **the disagreement case**, and the one that used to be a loop: a preprint server serving what the registries call the version of record. The resolver read the record; the route only knows it serves preprints. The resolver wins. |
    /// | `Some(preprint)` | `Arxiv` | `preprint` | the agreeing case, so the winner is not just "anything the caller says" |
    /// | `Some(Unknown)` | `Unpaywall` | `unknown` | *looked*, nothing stated — and still `unknown`, because the column has no other word for it |
    /// | `None` | `Unpaywall` | `unknown` | the route's fallback, which is the same word for a different reason |
    ///
    /// The last two are both `unknown` and that is the point of keeping them
    /// separate inputs: `Some(Unknown)` says the resolver asked, `None` says it
    /// did not, and collapsing them would make "we looked and got no answer"
    /// indistinguishable from "we never looked" in the recorded row.
    ///
    /// # Why this is not a licence-style hole (#261)
    ///
    /// `access_basis` is still derived from the route and still cannot be
    /// overridden, because it has one producer. Version has two, and the second
    /// is entitled to answer — but the field takes a closed
    /// [`ArtefactVersion`] and nothing else, so there is nowhere for a *guess*
    /// to go. `the_route_still_pairs_with_its_own_licence_answer` below holds
    /// the other half of that.
    #[test]
    fn the_resolvers_version_outranks_the_routes_fallback() {
        for (id, route, caller, expected) in [
            (
                "p-disagree",
                RouteId::Arxiv,
                Some(ArtefactVersion::VersionOfRecord),
                "vor",
            ),
            (
                "p-agree",
                RouteId::Arxiv,
                Some(ArtefactVersion::Preprint),
                "preprint",
            ),
            (
                "p-author-manuscript",
                RouteId::Unpaywall,
                Some(ArtefactVersion::AuthorManuscript),
                "am",
            ),
            (
                "p-looked-not-stated",
                RouteId::Unpaywall,
                Some(ArtefactVersion::Unknown),
                "unknown",
            ),
            ("p-fell-back", RouteId::Unpaywall, None, "unknown"),
        ] {
            // `_dir` is bound and kept for the whole block: the pool opens
            // connections lazily, so dropping the directory would let a later
            // `conn()` create an empty, unmigrated database.
            let (_dir, db) = library_with(id);
            let mut conn = db.conn().expect("conn");
            record_download(
                &mut conn,
                &DownloadWrite {
                    paper_id: id.into(),
                    route,
                    version: caller,
                    ext: "pdf".into(),
                    access_status: AccessStatus::FullText,
                    source_url: None,
                    publisher: None,
                    publisher_note: None,
                    // `None`: this table is the version precedence, and the
                    // licence has its own test below.
                    licence: None,
                    retrieved_at: NOW.into(),
                    blob: Some(BlobWrite {
                        sha256: format!("{id}-sha"),
                        bytes: 1,
                        mime: "application/pdf".into(),
                        rel_path: format!("blobs/{id}.pdf"),
                        created_at: NOW.into(),
                    }),
                },
            )
            .unwrap_or_else(|e| panic!("{id}: {e}"));

            let row: (String, String, String) = conn
                .query_row(
                    "SELECT version, route, access_basis FROM artefacts WHERE paper_id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap_or_else(|e| panic!("{id}: read back: {e}"));
            assert_eq!(
                row.0, expected,
                "{id}: route {route} + caller {caller:?} must write {expected:?}, got {:?}",
                row.0
            );
            assert_eq!(
                row.1,
                route.label(),
                "{id}: the route is recorded whatever the version was"
            );
            assert_eq!(
                row.2,
                route.access_basis().expect("a basis"),
                "{id}: and the licence is still the route's own answer — a caller \
                 that may set a version may not pair a route with another \
                 route's licence"
            );
        }
    }

    /// A version a caller sets must not be able to smuggle in a value migration
    /// 013 would have refused. The field is typed, so this is a compile-time
    /// guarantee rather than a runtime one — the test exists to say so, and to
    /// fail loudly if the field is ever widened to a string.
    #[test]
    fn a_caller_supplied_version_cannot_leave_the_check_vocabulary() {
        for version in ArtefactVersion::ALL {
            assert!(
                ["vor", "am", "preprint", "unknown"].contains(&version.label()),
                "{version:?} writes {:?}, which is not a migration-013 value",
                version.label()
            );
        }
        assert_eq!(
            ArtefactVersion::parse("vor-and-am"),
            None,
            "and a value outside it does not parse rather than defaulting"
        );
    }

    /// A row with real bytes: `sha256` is a foreign key into `blobs`, so
    /// a row that claims content must bring the blob with it.
    fn row(kind: &str, locator: &str, sha: &str) -> ArtefactWrite {
        ArtefactWrite {
            id: String::new(),
            paper_id: "p-1".into(),
            kind: kind.into(),
            version: VERSION_UNKNOWN.into(),
            locator: locator.into(),
            sha256: Some(sha.into()),
            format: Some("pdf".into()),
            access_status: "full_text".into(),
            route: ROUTE_LEGACY.into(),
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
            imported_from: Some("/tmp/x.pdf".into()),
            retrieved_at: "2026-01-01T00:00:00+00:00".into(),
            missing_on_disk: false,
            blob: Some(BlobWrite {
                // Not a real digest: `blobs.sha256` is TEXT and nothing
                // checks its shape.
                sha256: sha.into(),
                bytes: 4,
                mime: "application/pdf".into(),
                rel_path: format!("blobs/aa/{sha}.pdf"),
                created_at: "2026-01-01T00:00:00+00:00".into(),
            }),
        }
    }

    /// A migrated file-backed database. The `TempDir` is returned
    /// alongside it because the pool opens connections lazily: dropping
    /// the directory mid-test would let a later `conn()` create a fresh,
    /// empty database file.
    fn open() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("scitadel.db")).unwrap();
        db.migrate().unwrap();
        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO papers (id, title, authors, created_at, updated_at)
             VALUES ('p-1', 't', '[]', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
            [],
        )
        .unwrap();
        drop(conn);
        (dir, db)
    }

    /// One artefact row as read back by a reader, for the provenance tests below.
    fn read_one(db: &Database, kind: &str, locator: &str) -> crate::sqlite::artefacts::ArtefactRow {
        read_artefacts_for_paper(&db.conn().unwrap(), "p-1")
            .unwrap()
            .into_iter()
            .find(|r| r.kind == kind && r.locator == locator)
            .unwrap_or_else(|| panic!("no {kind}/{locator} artefact"))
    }

    /// A figure row carrying a `label` and a `caption` — the two fields whose
    /// content a publisher wrote and scitadel stores verbatim.
    fn captioned_figure(label: &str, caption: &str) -> ArtefactWrite {
        let mut row = row("figure", "fig-2", "aaa");
        // A figure reference: no bytes, which is what the ADR allows for a
        // `sha256 IS NULL` figure row and what a caption-only artefact is.
        row.sha256 = None;
        row.blob = None;
        row.format = None;
        row.access_status = "unknown".into();
        row.label = Some(label.into());
        row.caption = Some(caption.into());
        row
    }

    // ---------- ADR-007 §1's untrusted-content envelope on artefact titles ----------

    /// The security-relevant one, at the artefact level: a caption written by
    /// whoever produced the file must come back out of the database unable to
    /// drive a terminal.
    ///
    /// The three shapes a caption can be hostile in — an OSC 8 hyperlink, a CSI
    /// that repaints, an OSC 0 that renames the window — are all written into the
    /// row the way a real importer writes them (`meta.json`, `tables.json`, a
    /// `.caption.txt` sidecar), read back, and rendered. Both the raw stored bytes
    /// and the rendered form are asserted, because the interesting failure is
    /// "the escape was stripped" *and* "the text is still there": a
    /// neutraliser that redacted instead would pass the first and fail a reader.
    #[test]
    fn an_artefact_title_with_terminal_escapes_is_neutralised() {
        use scitadel_core::untrusted::Provenance;

        let hostile = concat!(
            "Figure 2: the uptake curve",
            "\u{1b}[31m",
            "\u{1b}[2J\u{1b}[H",
            "\u{1b}]8;;https://attacker.example/\u{1b}\\see the data\u{1b}]8;;\u{1b}\\",
            "\u{1b}]0;scitadel\u{7}",
            "\u{202e}reversed caption",
        );
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        write_artefacts(
            &mut conn,
            &[captioned_figure("Figure 2", hostile)],
            WriteMode::Reconcile,
        )
        .unwrap();

        let stored = read_one(&db, "figure", "fig-2");
        assert_eq!(
            stored.caption.as_deref(),
            Some(hostile),
            "what was written is what is stored: the mirror's fidelity is not \
             display safety"
        );

        let caption = stored.caption_text().expect("a caption");
        assert_eq!(caption.as_str(), hostile);
        assert_eq!(
            caption.provenance(),
            Provenance::PublisherSupplied,
            "a caption is the document's own sentence about a figure"
        );

        let rendered = caption.rendered().into_owned();
        assert!(
            !rendered.contains('\u{1b}'),
            "no escape byte survives a render: {rendered:?}"
        );
        for needle in ["attacker.example", "scitadel", "\u{202e}", "[2J"] {
            assert!(!rendered.contains(needle), "{needle} must not survive");
        }
        assert!(
            rendered.starts_with("Figure 2: the uptake curve see the data reversed caption"),
            "the words survive, in order, as one line: {rendered:?}"
        );

        // `Display` is the neutralised form, so `{}` cannot reach a terminal.
        assert_eq!(caption.to_string(), rendered);
        assert_eq!(format!("{caption}"), rendered);

        // And the label — the other title-bearing column, whose content is a
        // filename somebody chose — is neutralised through the same accessor.
        let label = stored.label_text().expect("a label");
        assert_eq!(label.as_str(), "Figure 2");
        assert_eq!(label.rendered(), "Figure 2");
        assert!(!label.provenance().is_trusted());
    }

    /// The cap, at the artefact level: a `tables.json` caption is unbounded and
    /// every surface that shows one is finite, so the boundary has to hold for the
    /// string a reader actually receives.
    #[test]
    fn an_artefact_title_is_capped_in_length() {
        use scitadel_core::untrusted::{MAX_RENDERED_CHARS, MAX_UNTRUSTED_CHARS};

        let caption = "Supplementary table of uptake kinetics. ".repeat(40);
        assert!(
            caption.chars().count() > MAX_UNTRUSTED_CHARS,
            "the fixture has to exceed the cap, or the test proves nothing"
        );
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        write_artefacts(
            &mut conn,
            &[captioned_figure("Table S1", &caption)],
            WriteMode::Reconcile,
        )
        .unwrap();

        let stored = read_one(&db, "figure", "fig-2");
        let rendered = stored
            .caption_text()
            .expect("a caption")
            .rendered()
            .into_owned();
        assert_eq!(rendered.chars().count(), MAX_RENDERED_CHARS);
        assert!(
            rendered.starts_with("Supplementary table of uptake kinetics. Supplementary"),
            "the beginning of the caption, in order: {rendered:?}"
        );
        assert!(
            !rendered.contains("  "),
            "and the whitespace collapse has not introduced a run either"
        );
        assert_eq!(
            stored
                .caption
                .as_deref()
                .map(str::chars)
                .map(Iterator::count),
            Some(caption.chars().count()),
            "the stored caption is untouched — the cap is a rendering limit"
        );
    }

    /// The marker is recoverable at the call site, which is what makes it a
    /// boundary: a consumer does not have to know which writer filled a row to know
    /// whether it is looking at scitadel's own words.
    ///
    /// Two rows, one table, two provenances — and the contrast with the identity
    /// check's two titles, where the *document's* title is
    /// `PublisherSupplied` and the one we compared it against is `Ours`
    /// (`IdentityCheckRow::resolved_title_text` /
    /// `expected_title_text`).
    #[test]
    fn provenance_distinguishes_our_title_from_a_publisher_supplied_one() {
        use scitadel_core::untrusted::{Provenance, UntrustedText};

        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        write_artefacts(
            &mut conn,
            &[captioned_figure(
                "Figure 2",
                "Uptake curve for the labelled cohort",
            )],
            WriteMode::Reconcile,
        )
        .unwrap();
        let stored = read_one(&db, "figure", "fig-2");

        let from_the_document = stored.caption_text().expect("a caption");
        let ours = UntrustedText::ours("Uptake curve for the labelled cohort");

        assert_eq!(
            from_the_document.as_str(),
            ours.as_str(),
            "identical words: provenance is not a guess about the content"
        );
        assert_eq!(
            from_the_document.provenance(),
            Provenance::PublisherSupplied
        );
        assert_eq!(ours.provenance(), Provenance::Ours);
        assert!(!from_the_document.is_trusted());
        assert!(ours.is_trusted());
        assert_ne!(
            from_the_document.provenance().label(),
            ours.provenance().label(),
            "and a report can say which is which"
        );
        assert_ne!(from_the_document.provenance(), ours.provenance());

        // Both render the same way, because `Ours` is a statement about who chose
        // the string rather than a licence to skip the escaping.
        assert_eq!(from_the_document.rendered(), ours.rendered());

        // A row with no title says so, rather than offering an empty string a
        // caller might render as a blank cell and read as a value.
        let mut conn = db.conn().unwrap();
        write_artefacts(
            &mut conn,
            &[row("fulltext_pdf", FULLTEXT_LOCATOR, "bbb")],
            WriteMode::Reconcile,
        )
        .unwrap();
        let bare = read_one(&db, "fulltext_pdf", FULLTEXT_LOCATOR);
        assert_eq!(bare.caption, None);
        assert_eq!(bare.caption_text(), None);
        assert_eq!(bare.label_text(), None);
    }

    /// One artefact row as a named tuple, so the tests above can say which
    /// column they mean.
    #[derive(Debug, PartialEq, Eq)]
    struct ArtefactRow {
        kind: String,
        locator: String,
        sha256: Option<String>,
        retrieved_at: String,
        route: String,
        access_basis: String,
        version: String,
        format: Option<String>,
        access_status: String,
        imported_from: Option<String>,
    }

    fn artefacts_of(db: &Database) -> Vec<ArtefactRow> {
        let conn = db.conn().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT kind, locator, sha256, retrieved_at, route, access_basis, version,
                        format, access_status, imported_from
                 FROM artefacts ORDER BY kind, locator",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok(ArtefactRow {
                kind: r.get(0)?,
                locator: r.get(1)?,
                sha256: r.get(2)?,
                retrieved_at: r.get(3)?,
                route: r.get(4)?,
                access_basis: r.get(5)?,
                version: r.get(6)?,
                format: r.get(7)?,
                access_status: r.get(8)?,
                imported_from: r.get(9)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    fn state_of(db: &Database) -> (String, String) {
        let conn = db.conn().unwrap();
        conn.query_row(
            "SELECT status, updated_at FROM acquisition_state",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn write_ignore_existing_leaves_a_conflicting_row_alone() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        write_artefacts(
            &mut conn,
            &[row("fulltext_pdf", "", "aaa")],
            WriteMode::IgnoreExisting,
        )
        .unwrap();
        write_artefacts(
            &mut conn,
            &[row("fulltext_pdf", "", "bbb")],
            WriteMode::IgnoreExisting,
        )
        .unwrap();
        drop(conn);

        let rows = artefacts_of(&db);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].sha256.as_deref(),
            Some("aaa"),
            "the first write wins"
        );
    }

    /// Reconcile follows a changed file, but an unchanged re-run leaves
    /// the row — including `retrieved_at`, which callers set from the
    /// file's own mtime rather than from the clock.
    #[test]
    fn write_reconcile_updates_only_what_changed() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        write_artefacts(
            &mut conn,
            &[row("fulltext_pdf", "", "aaa")],
            WriteMode::Reconcile,
        )
        .unwrap();
        let first = artefacts_of(&db);

        // Byte-for-byte the same tree: no row changes at all.
        write_artefacts(
            &mut conn,
            &[row("fulltext_pdf", "", "aaa")],
            WriteMode::Reconcile,
        )
        .unwrap();
        assert_eq!(artefacts_of(&db), first, "an unchanged re-run is a no-op");

        // New bytes: the row follows them.
        write_artefacts(
            &mut conn,
            &[row("fulltext_pdf", "", "bbb")],
            WriteMode::Reconcile,
        )
        .unwrap();
        let after = artefacts_of(&db);
        assert_eq!(after[0].sha256.as_deref(), Some("bbb"));
        assert_eq!(
            after[0].locator, first[0].locator,
            "locator is the stable key"
        );
    }

    /// `retrieved_at` is *when the content was retrieved*, not when the
    /// row was last touched — so a file whose mtime moved is recorded as
    /// newly retrieved even if the bytes happen to be identical.
    #[test]
    fn write_reconcile_treats_a_new_retrieval_time_as_a_change() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        write_artefacts(
            &mut conn,
            &[row("fulltext_pdf", "", "aaa")],
            WriteMode::Reconcile,
        )
        .unwrap();

        let mut later = row("fulltext_pdf", "", "aaa");
        later.retrieved_at = "2027-06-06T06:06:06+00:00".into();
        write_artefacts(&mut conn, &[later], WriteMode::Reconcile).unwrap();
        assert_eq!(
            artefacts_of(&db)[0].retrieved_at,
            "2027-06-06T06:06:06+00:00"
        );
    }

    // ---------- record_download: the only shape ----------

    /// A download of `route` for `p-1`, with one content digest.
    fn download(route: RouteId, sha: &str, retrieved_at: &str) -> DownloadWrite {
        DownloadWrite {
            paper_id: "p-1".into(),
            route,
            // `None`: the fixture predates the resolve-then-rank version, and the
            // route's static claim is the correct answer for a download that
            // established nothing. `the_resolvers_version_outranks_the_routes_fallback`
            // is the test that sets it.
            version: None,
            ext: "pdf".into(),
            access_status: AccessStatus::FullText,
            source_url: Some("https://example.org/paper.pdf".into()),
            publisher: Some("nature".into()),
            publisher_note: None,
            // `None`: the fixture predates the resolve pass's licence
            // attribution, and "this source named no licence" is the correct
            // answer for a download that established none.
            // `the_licence_the_resolver_attributed_is_written_beside_the_bytes`
            // is the test that sets it.
            licence: None,
            retrieved_at: retrieved_at.into(),
            blob: Some(BlobWrite {
                sha256: sha.into(),
                bytes: 4,
                mime: "application/pdf".into(),
                rel_path: format!("blobs/{}/{sha}.pdf", &sha[..2]),
                created_at: retrieved_at.into(),
            }),
        }
    }

    /// `(local_path, download_status, last_attempt_at)` for `p-1`.
    ///
    /// The only reader of the three retired columns left in the workspace, and it
    /// is a test: the S1 backfill (`sqlite::acquisition`) is the production one,
    /// and it exists precisely to migrate an existing library. Everything else
    /// reads artefacts.
    fn legacy_of(db: &Database) -> (Option<String>, Option<String>, Option<String>) {
        let conn = db.conn().unwrap();
        conn.query_row(
            "SELECT local_path, download_status, last_attempt_at FROM papers WHERE id = 'p-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    }

    /// #253's S2e: a recorded download writes the artefact row and nothing else.
    ///
    /// This replaces `a_download_records_the_artefact_and_the_legacy_columns_together`,
    /// which asserted the opposite — that one transaction wrote `local_path`,
    /// `download_status` and `last_attempt_at` beside the artefact. That test was
    /// S1's promise ("no behaviour change") and it is now the bug: those columns
    /// were a second answer to "do we have this paper", and a second answer is
    /// what goes stale. A re-download through a different route moved the artefact
    /// and left `local_path` naming the file from the fetch before it, so `O` in
    /// the TUI opened the previous paper.
    ///
    /// The assertion is `NULL` on all three rather than "unchanged", because the
    /// fixture's row has never been written: a test that recorded a value first and
    /// asserted it survived would pass against an implementation that never wrote
    /// the column in the first place, which is the half that matters.
    #[test]
    fn the_legacy_columns_are_no_longer_written() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        record_download(
            &mut conn,
            &download(RouteId::Arxiv, "aaa", "2026-01-01T00:00:00+00:00"),
        )
        .unwrap();

        let rows = artefacts_of(&db);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].kind, "fulltext_pdf");
        assert_eq!(rows[0].format.as_deref(), Some("pdf"));
        assert_eq!(rows[0].route, "arxiv");
        assert_eq!(
            rows[0].access_basis, "oa_license",
            "the basis comes from the route, not from the caller"
        );
        assert_eq!(rows[0].version, "preprint");
        assert_eq!(rows[0].access_status, "full_text");
        assert_eq!(rows[0].sha256.as_deref(), Some("aaa"));
        assert_eq!(rows[0].imported_from, None, "a fetch imports nothing");

        // The three retired columns are untouched. See the test's own docs for why
        // this replaces the dual-write assertion rather than adding to it.
        assert_eq!(
            legacy_of(&db),
            (None, None, None),
            "a recorded download writes no local_path, no download_status and no \
             last_attempt_at; ADR-007 §1's derivation is the only reader left, and \
             the columns are dropped a release later"
        );

        let blobs: i64 = {
            let conn = db.conn().unwrap();
            conn.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(blobs, 1);
    }

    /// #291: the artefact used to record *why* access is lawful and not *which
    /// licence* — the four `license_*` columns migration 013 added had no writer
    /// at all, so the manifest mirror reported `null` for every licence forever.
    ///
    /// The rule this pins: the four columns are a **verbatim copy** of the offer
    /// the resolver attributed to the chosen candidate, and an absent field
    /// stays absent. `None` on all four is "no licence was recorded", which is
    /// not "there is no licence" — and `access_basis` alone still answers why
    /// the bytes could be fetched, which is the whole of the row's claim for a
    /// work nobody published terms for.
    #[test]
    fn the_licence_the_resolver_attributed_is_written_beside_the_bytes() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();

        // Unpaywall served the bytes, and the grant the copy carries was
        // **Crossref's statement about the work**, filtered to the version
        // this candidate was ranked at — which is why the version is set here
        // and why a `vor` grant beside a `preprint` row is a shape this writer
        // refuses to produce rather than one it silently allows.
        let mut licensed = download(RouteId::Unpaywall, "aaa", "2026-01-01T00:00:00+00:00");
        licensed.version = Some(ArtefactVersion::VersionOfRecord);
        licensed.licence = Some(LicenceWrite {
            url: "https://creativecommons.org/licenses/by/4.0/".to_string(),
            content_version: Some("vor".to_string()),
            start: Some("2024-01-01".to_string()),
            source: "crossref".to_string(),
        });
        record_download(&mut conn, &licensed).expect("the licensed download");

        // An unlicensed fetch of a *different* work, through a route that
        // establishes a basis but states no grant — arXiv's own shape.
        let mut unlicensed = download(RouteId::Arxiv, "bbb", "2026-01-01T00:00:00+00:00");
        unlicensed.paper_id = "p-2".into();
        conn.execute(
            "INSERT INTO papers (id, title, authors, created_at, updated_at)
             VALUES ('p-2', 'Another work', '[]', '2026-01-01T00:00:00+00:00',
                     '2026-01-01T00:00:00+00:00')",
            [],
        )
        .unwrap();
        record_download(&mut conn, &unlicensed).expect("the unlicensed download");

        let licence_of = |paper_id: &str| -> (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) {
            let conn = db.conn().unwrap();
            conn.query_row(
                "SELECT license_url, license_content_version, license_start, license_source
                   FROM artefacts WHERE paper_id = ?1",
                [paper_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap_or_else(|e| panic!("{paper_id}: read back: {e}"))
        };

        assert_eq!(
            licence_of("p-1"),
            (
                Some("https://creativecommons.org/licenses/by/4.0/".to_string()),
                Some("vor".to_string()),
                Some("2024-01-01".to_string()),
                Some("crossref".to_string()),
            ),
            "copied verbatim: a mirror that has to reconstruct these cannot \
             report a licence nobody wrote down"
        );
        assert_eq!(
            licence_of("p-2"),
            (None, None, None, None),
            "no offer named, no columns written — NULL, not a guess and not an \
             empty string a reader would render as a blank cell"
        );

        let (version, basis): (String, String) = conn
            .query_row(
                "SELECT version, access_basis FROM artefacts WHERE paper_id = 'p-2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            version, "preprint",
            "and the preprint row carries no `vor` grant beside it: the offer is \
             filtered to the candidate's version before it is ever stored"
        );
        assert_eq!(
            basis, "oa_license",
            "while access_basis is unchanged: the route's own answer is what \
             records why an unlicensed fetch was lawful"
        );
    }

    /// A re-download moves the licence with the bytes, exactly as it moves
    /// `route` and `access_basis`.
    ///
    /// The failure this prevents is the subtle one: a first fetch through a
    /// source that named CC-BY, a re-download through a source that named
    /// nothing, and a row left claiming the first source's grant beside the
    /// second source's bytes. That is a licence attributed to bytes nobody
    /// licensed, which is the overclaim #261 exists to stop — reached through
    /// an upsert rather than through a caller.
    #[test]
    fn a_re_download_moves_the_licence_with_the_bytes() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        let mut first = download(RouteId::Unpaywall, "aaa", "2026-01-01T00:00:00+00:00");
        first.licence = Some(LicenceWrite {
            url: "https://creativecommons.org/licenses/by/4.0/".to_string(),
            content_version: None,
            start: None,
            source: "unpaywall".to_string(),
        });
        record_download(&mut conn, &first).unwrap();
        let before: Vec<crate::sqlite::artefacts::ArtefactRow> =
            read_artefacts_for_paper(&conn, "p-1").unwrap();
        assert_eq!(
            before[0].license_url.as_deref(),
            Some("https://creativecommons.org/licenses/by/4.0/"),
            "the first fetch recorded the grant the naming source stated"
        );

        // Same UNIQUE key (both `unknown`), so this is an update and not a
        // second row — and this source named no licence.
        record_download(
            &mut conn,
            &download(RouteId::Publisher, "bbb", "2027-06-06T06:06:06+00:00"),
        )
        .unwrap();

        let rows: Vec<crate::sqlite::artefacts::ArtefactRow> =
            read_artefacts_for_paper(&conn, "p-1").unwrap();
        assert_eq!(rows.len(), 1, "same (paper, kind, version, locator)");
        assert_eq!(
            rows[0].route, "publisher",
            "the row followed the route that served the second fetch"
        );
        assert_eq!(
            (
                rows[0].license_url.as_deref(),
                rows[0].license_content_version.as_deref(),
                rows[0].license_start.as_deref(),
                rows[0].license_source.as_deref(),
            ),
            (None, None, None, None),
            "the re-download's silence takes the grant away rather than leaving \
             the previous source's URL beside bytes it was never said about"
        );
        assert_eq!(
            rows[0].access_basis, "subscription_read",
            "and the basis moved too — the two answers travel together"
        );
    }

    /// A re-download *did* happen, so the row follows the new route rather
    /// than keeping the old answer beside new bytes. `Reconcile` would not
    /// do this: it deliberately leaves `route` alone, because a re-scanned
    /// file tree never changes provenance.
    #[test]
    fn a_re_download_moves_the_row_to_the_route_that_served_it() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        // Unpaywall -> Publisher: both `unknown` (so they share the UNIQUE
        // key), and they establish *different* access bases, so this moves
        // the route and the licence answer together.
        record_download(
            &mut conn,
            &download(RouteId::Unpaywall, "aaa", "2026-01-01T00:00:00+00:00"),
        )
        .unwrap();
        assert_eq!(artefacts_of(&db)[0].route, "unpaywall");

        record_download(
            &mut conn,
            &download(RouteId::Publisher, "bbb", "2027-06-06T06:06:06+00:00"),
        )
        .unwrap();

        let rows = artefacts_of(&db);
        assert_eq!(rows.len(), 1, "same (paper, kind, version, locator)");
        assert_eq!(rows[0].route, "publisher", "the route moves with the bytes");
        assert_eq!(
            rows[0].access_basis, "subscription_read",
            "and so does the basis that route established"
        );
        assert_eq!(rows[0].sha256.as_deref(), Some("bbb"));
        assert_eq!(rows[0].retrieved_at, "2027-06-06T06:06:06+00:00");
    }

    /// A download that collides with a row the backfill or the flat
    /// importer left behind takes the row over — and takes its
    /// `imported_from` with it. Citing the old file's absolute path beside
    /// bytes fetched from a publisher would be a provenance lie, and
    /// absolute paths break when a library moves anyway.
    #[test]
    fn a_download_over_an_imported_row_takes_its_provenance_with_it() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        // A backfilled legacy row: `unknown` version, manual basis, and an
        // absolute `imported_from` — exactly what `write_artefacts` in
        // `IgnoreExisting` leaves behind.
        write_artefacts(
            &mut conn,
            &[row("fulltext_pdf", FULLTEXT_LOCATOR, "aaa")],
            WriteMode::IgnoreExisting,
        )
        .unwrap();
        assert_eq!(
            artefacts_of(&db)[0].imported_from.as_deref(),
            Some("/tmp/x.pdf"),
            "the import recorded where the file was"
        );

        record_download(
            &mut conn,
            &download(RouteId::Unpaywall, "bbb", "2026-01-01T00:00:00+00:00"),
        )
        .unwrap();

        let rows = artefacts_of(&db);
        assert_eq!(rows.len(), 1, "same (paper, kind, version, locator)");
        assert_eq!(rows[0].route, "unpaywall");
        assert_eq!(
            rows[0].imported_from, None,
            "the fetched row cites no imported path"
        );
    }

    /// `version` is part of the UNIQUE key, so a re-download that *changes*
    /// the version is a **second** artefact rather than an update — which is
    /// the point: an arXiv preprint and a publisher's version of record are
    /// two different things we hold, and neither may be overwritten by the
    /// other.
    #[test]
    fn a_re_download_that_changes_the_version_adds_an_artefact() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        record_download(
            &mut conn,
            &download(RouteId::Unpaywall, "aaa", "2026-01-01T00:00:00+00:00"),
        )
        .unwrap();
        record_download(
            &mut conn,
            &download(RouteId::Arxiv, "bbb", "2027-06-06T06:06:06+00:00"),
        )
        .unwrap();

        let mut rows = artefacts_of(&db);
        assert_eq!(rows.len(), 2, "{rows:?}");
        rows.sort_by(|a, b| a.version.cmp(&b.version));
        assert_eq!(rows[0].version, "preprint");
        assert_eq!(rows[1].version, "unknown");
        let blobs: i64 = {
            let conn = db.conn().unwrap();
            conn.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(blobs, 2, "two byte sequences, two blobs");
    }

    /// `version` and `access_basis` are `NOT NULL`, and neither is a
    /// defaultable value: "this route establishes no basis" has no
    /// spelling. A pseudo-route (no version) and a discovery route (no
    /// basis) therefore have to be refused rather than defaulted.
    #[test]
    fn a_route_that_establishes_nothing_is_refused_not_defaulted() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();

        // A pseudo-route: `never_fetches()` is true, so no version exists.
        let err = record_download(
            &mut conn,
            &download(RouteId::Legacy, "aaa", "2026-01-01T00:00:00+00:00"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("legacy"), "{err}");

        // A discovery route: it answers "where do the bytes live", not
        // "may we keep them", so `access_basis()` is `None`.
        let err = record_download(
            &mut conn,
            &download(RouteId::Crossref, "aaa", "2026-01-01T00:00:00+00:00"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("access_basis"), "{err}");

        assert!(
            artefacts_of(&db).is_empty(),
            "a refused write leaves nothing behind"
        );
    }

    /// `artefacts.kind` is a closed vocabulary, so an extension outside it
    /// gets no row rather than a reclassified one.
    #[test]
    fn an_extension_outside_the_kind_vocabulary_is_refused() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        let mut write = download(RouteId::Arxiv, "aaa", "2026-01-01T00:00:00+00:00");
        write.ext = "docx".into();
        let err = record_download(&mut conn, &write).unwrap_err();
        assert!(err.to_string().contains("docx"), "{err}");
        assert!(artefacts_of(&db).is_empty());
    }

    /// A work that is not in `papers` cannot be recorded — the two shapes
    /// are one atomic fact, so there is no such thing as "the artefact row
    /// but not the legacy columns".
    #[test]
    fn a_download_for_a_work_that_does_not_exist_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("scitadel.db")).unwrap();
        db.migrate().unwrap();
        let mut conn = db.conn().unwrap();

        // `PRAGMA foreign_keys` is on, so the artefacts insert itself is
        // what refuses — the `updated != 1` check behind it is defence in
        // depth for a connection without the pragma. Either way the answer
        // is the same, and the message names the constraint rather than the
        // paper.
        let err = record_download(
            &mut conn,
            &download(RouteId::Arxiv, "aaa", "2026-01-01T00:00:00+00:00"),
        )
        .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("foreign key"),
            "{err}"
        );

        let artefacts: i64 = conn
            .query_row("SELECT COUNT(*) FROM artefacts", [], |r| r.get(0))
            .unwrap();
        let blobs: i64 = conn
            .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(artefacts, 0, "no partial artefact row");
        assert_eq!(blobs, 0, "and the blobs row rolled back with it");
    }

    /// Two tables coexist only because `locator` differs.
    #[test]
    fn distinct_locators_produce_distinct_rows_under_the_unique_key() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        write_artefacts(
            &mut conn,
            &[
                row("table", "table-1", "aaa"),
                row("table", "table-2", "bbb"),
            ],
            WriteMode::Reconcile,
        )
        .unwrap();
        drop(conn);

        let rows = artefacts_of(&db);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter().map(|r| r.locator.as_str()).collect::<Vec<_>>(),
            vec!["table-1", "table-2"]
        );
    }

    #[test]
    fn state_rows_upsert_without_rewriting_an_identical_row() {
        let (_dir, db) = open();
        let gap = StateWrite {
            paper_id: "p-1".into(),
            kind: "fulltext".into(),
            locator: String::new(),
            wanted_version: "vor".into(),
            status: "pending".into(),
            reason: Some("nothing there".into()),
            publisher: None,
            hint_url: None,
            drop_path: Some("/lib/papers/stem/fulltext.pdf".into()),
            next_attempt_at: None,
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        };
        {
            let mut conn = db.conn().unwrap();
            write_acquisition_states(&mut conn, std::slice::from_ref(&gap)).unwrap();
        }
        let first = state_of(&db);

        // Same gap, re-run much later: the row keeps its original stamp.
        let mut again = gap.clone();
        again.updated_at = "2027-06-06T06:06:06+00:00".into();
        {
            let mut conn = db.conn().unwrap();
            write_acquisition_states(&mut conn, &[again]).unwrap();
        }
        assert_eq!(state_of(&db), first, "an unchanged gap is not rewritten");

        // The gap closes — but only a row we wrote is ours to delete.
        {
            let conn = db.conn().unwrap();
            assert_eq!(
                delete_acquisition_states_at(&conn, "p-1", "/somewhere/else.pdf").unwrap(),
                0,
                "a row we did not write is not ours to delete"
            );
            assert_eq!(
                delete_acquisition_states_at(&conn, "p-1", gap.drop_path.as_deref().unwrap())
                    .unwrap(),
                1
            );
        }
        let conn = db.conn().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM acquisition_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    /// The `WHERE` guard is load-bearing for the two columns ADR-007 §2 needs
    /// and `StateWrite` used to have no way to write: a row whose *only* change
    /// is `next_attempt_at` (the retry time moving while the status stands
    /// still) or `publisher` has to update. Excluded from the guard, the
    /// upsert is a silent no-op — `ON CONFLICT DO UPDATE ... WHERE <false>`
    /// touches nothing — and `--resume` would then re-fetch a work it had
    /// already deferred, forever.
    #[test]
    fn a_change_to_only_next_attempt_at_or_publisher_updates_the_row() {
        let (_dir, db) = open();
        let base = StateWrite {
            paper_id: "p-1".into(),
            kind: "fulltext".into(),
            locator: String::new(),
            wanted_version: "vor".into(),
            status: "error".into(),
            reason: Some("the OA host could not be resolved".into()),
            publisher: None,
            hint_url: None,
            drop_path: None,
            next_attempt_at: None,
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        };
        let mut conn = db.conn().unwrap();
        write_acquisition_states(&mut conn, std::slice::from_ref(&base)).unwrap();

        let stamp = |db: &Database| -> (Option<String>, Option<String>, String) {
            let conn = db.conn().unwrap();
            conn.query_row(
                "SELECT publisher, next_attempt_at, updated_at FROM acquisition_state",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };
        assert_eq!(stamp(&db), (None, None, base.updated_at.clone()));

        // Only `next_attempt_at` moves. Nothing else differs — including
        // `updated_at`, which the caller wrote identically.
        let deferred = StateWrite {
            next_attempt_at: Some("2026-01-01T00:10:00+00:00".into()),
            ..base.clone()
        };
        write_acquisition_states(&mut conn, &[deferred]).unwrap();
        assert_eq!(
            stamp(&db),
            (
                None,
                Some("2026-01-01T00:10:00+00:00".to_string()),
                base.updated_at.clone()
            ),
            "a row whose only change is next_attempt_at must update"
        );

        // Only `publisher` moves — the classified-after-the-fact case.
        let classified = StateWrite {
            publisher: Some("springer".into()),
            next_attempt_at: Some("2026-01-01T00:10:00+00:00".into()),
            ..base.clone()
        };
        write_acquisition_states(&mut conn, &[classified]).unwrap();
        assert_eq!(
            stamp(&db),
            (
                Some("springer".to_string()),
                Some("2026-01-01T00:10:00+00:00".to_string()),
                base.updated_at.clone()
            ),
            "a row whose only change is publisher must update"
        );
    }

    /// #260: the acquisition ladder records a gap when a walk ends without
    /// bytes, so a *later* successful download has to take it back — in the
    /// same transaction, or a reader in between sees a work that is both held
    /// and wanted, which is the divergence `acquisition_state` exists to
    /// prevent.
    ///
    /// The three rows are the three ways this table gets written, and only the
    /// first is the ladder's to retract: the importer's row carries a
    /// `drop_path` (it is closed by that file arriving, not by a fetch), and
    /// the SI want is a different `kind` that a full-text download does not
    /// satisfy.
    #[test]
    fn a_download_retracts_the_full_text_want_it_satisfied() {
        let (_dir, db) = open();
        let mut conn = db.conn().unwrap();
        for (kind, locator, drop_path) in [
            ("fulltext", "", None),
            ("fulltext", "", Some("/lib/papers/stem/fulltext.pdf")),
            ("si", "table-1", None),
        ] {
            write_acquisition_states(
                &mut conn,
                &[StateWrite {
                    paper_id: "p-1".into(),
                    kind: kind.into(),
                    locator: locator.into(),
                    wanted_version: "vor".into(),
                    status: "unavailable".into(),
                    reason: Some("every route that could be tried refused".into()),
                    publisher: None,
                    hint_url: Some("https://doi.org/10.1038/s41586-020-2649-2".into()),
                    drop_path: drop_path.map(str::to_string),
                    next_attempt_at: None,
                    updated_at: "2026-01-01T00:00:00+00:00".into(),
                }],
            )
            .unwrap();
        }

        record_download(
            &mut conn,
            &download(RouteId::Biorxiv, "aaa", "2026-01-01T00:00:00+00:00"),
        )
        .unwrap();

        let remaining: Vec<(String, String, Option<String>)> = {
            let conn = db.conn().unwrap();
            let mut stmt = conn
                .prepare("SELECT kind, locator, drop_path FROM acquisition_state ORDER BY kind, drop_path")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            remaining,
            vec![
                // The importer's row is left alone: it is closed by a file
                // arriving at its drop_path, not by this fetch.
                (
                    "fulltext".to_string(),
                    String::new(),
                    Some("/lib/papers/stem/fulltext.pdf".to_string())
                ),
                // And an SI want is not satisfied by a full text.
                ("si".to_string(), "table-1".to_string(), None),
            ],
            "only the ladder's own full-text want is retracted"
        );
        assert_eq!(
            artefacts_of(&db).len(),
            1,
            "and the artefact that satisfied it is recorded"
        );
    }

    #[test]
    fn the_id_is_a_pure_function_of_the_unique_key() {
        let id = artefact_id("p-1", "table", VERSION_UNKNOWN, "table-1");
        assert_eq!(id, artefact_id("p-1", "table", VERSION_UNKNOWN, "table-1"));
        assert_ne!(id, artefact_id("p-1", "table", VERSION_UNKNOWN, "table-2"));
        assert_ne!(id, artefact_id("p-2", "table", VERSION_UNKNOWN, "table-1"));
        // Length framing: ("ab","c") must not collide with ("a","bc").
        assert_ne!(
            artefact_id("ab", "c", "unknown", ""),
            artefact_id("a", "bc", "unknown", "")
        );
        assert_eq!(id.len(), 32);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
