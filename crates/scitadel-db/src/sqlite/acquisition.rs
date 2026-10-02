//! ADR-007 §1 "Legacy data": the backfill from today's three-column,
//! one-file-per-paper shape into `artefacts` + `blobs`.
//!
//! Migration 013 gave a work many files with provenance; what is on disk
//! today is one file per work recorded in `papers.local_path`, with
//! `download_status` saying whether it is the real thing and
//! `last_attempt_at` saying when we last tried. This module carries that
//! shape across without deleting any of it: the legacy columns stay (S1
//! promises no behaviour change, so `find_cached_file` and `read_paper`
//! keep reading them), and the files stay where they are — ADR-007 says
//! the backfill **copies** into `blobs/` and never moves.
//!
//! ## Two phases, deliberately
//!
//! 1. **File work with no write lock held**: read, hash and copy each
//!    legacy file into `<root>/blobs/<first 2 hex>/<sha256>.<ext>`. A
//!    failure here leaves the database completely untouched.
//! 2. **One short transaction** writing the `blobs` and `artefacts` rows.
//!
//! Same reason the file backfill runs after the migration's commit
//! (ADR-007 §1 "Schema"): the write lock stays short, well inside another
//! process's `busy_timeout`.
//!
//! ## Idempotence
//!
//! `INSERT OR IGNORE` against `UNIQUE (paper_id, kind, version,
//! locator)`, plus deterministic artefact ids (see [`artefact_id`]) and a
//! content-addressed blob store. A second run re-reads the same files,
//! re-derives the same ids and the same hash, finds the blob already in
//! place, and writes nothing — it does not even rewrite file bodies.
//!
//! ## The decision tables
//!
//! `kind` comes from the file extension, lowercased. The legacy writer
//! (`scitadel_adapters::download`) only ever produced `.pdf` and `.html`,
//! and `find_cached_file` only looks for those two, so those are the
//! cases that matter; `xml`/`nxml`/`jats` are included because JATS is a
//! real full-text serialisation and raid consumes it.
//!
//! | extension | `kind` | `format` | `mime` |
//! |---|---|---|---|
//! | `pdf` | `fulltext_pdf` | `pdf` | `application/pdf` |
//! | `html`, `htm`, `xhtml` | `fulltext_html` | `html` | `text/html` |
//! | `nxml`, `jats` | `fulltext_xml` | `jats` | `application/xml` |
//! | `xml` | `fulltext_xml` | `xml` | `application/xml` |
//! | anything else | *(no artefact — see below)* | | |
//!
//! An unrecognised extension gets **no** artefact and a `warn!` naming the
//! paper and path. `artefacts.kind` is a closed vocabulary of things the
//! "have" derivation understands, so filing a `.docx` as
//! `fulltext_html` would make coverage claim we hold a full text we
//! cannot read. The file is untouched, so nothing is lost — a later
//! `scitadel attach <paper> <file> --kind …` can place it properly.
//!
//! `access_status` comes from `papers.download_status`:
//!
//! | legacy `download_status` | `access_status` | why |
//! |---|---|---|
//! | `downloaded` | `full_text` | the adapter classified those bytes as full content |
//! | `paywall` | `paywall` | the file exists but is an abstract / stub / landing page — it is *not* the full text, and saying so is what keeps "have" honest |
//! | `failed` | `unknown` | a later attempt overwrote the status without clearing what we know about the file |
//! | absent | `unknown` | a hand-recorded path with no recorded outcome; ADR-007's rule for unknown manual drop-ins |
//!
//! Two of those refine ADR-007's own legacy table, deliberately:
//!
//! * ADR-007 maps `paywall` to `access_status = 'unknown'` on the
//!   grounds that "there is no real access status to preserve" — the TUI
//!   collapses Abstract / Paywall / Unknown into one `paywall` value
//!   (`app.rs` `persist_download_outcome`). We keep the collapsed value:
//!   the work *is* paywalled, and `access_status = 'paywall'` is excluded
//!   from the "have" derivation exactly like `unknown` is, so nothing
//!   downstream changes.
//! * ADR-007's last row says a recorded path that no longer exists gets
//!   *no* artefact. We record one with `sha256 = NULL` and
//!   `missing_on_disk = 1`, which is ADR-007's own rule for a file that
//!   disappeared (§1 "Artefact rules": "the row stays") and keeps the
//!   fact queryable rather than merely logged. `missing_on_disk = 1` here
//!   means "the path is recorded but we hold no usable bytes for it",
//!   which also covers a file we cannot read.
//!
//! Everything else is fixed by the ADR: `version = 'unknown'` (a legacy
//! file predates version tracking, so claiming `vor` would be a lie — and
//! §1 "Have" explicitly trusts `unknown` on `route IN
//! ('legacy','import_flat')` rows for exactly that reason), `route =
//! 'legacy'`, `access_basis = 'manual'`, `locator = ''`,
//! `imported_from` = the original absolute path (absolute paths break when
//! a library moves, so we record where the file *was*), and `retrieved_at`
//! from `last_attempt_at`, else `created_at`, else now.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::params;
use rusqlite::{Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::error::DbError;
use crate::sqlite::Database;

/// Route recorded on every row this module writes — one of the sentinel
/// values ADR-007 allows alongside `RouteId`s on `artefacts.route`.
const ROUTE_LEGACY: &str = "legacy";

/// ADR-007 §1 "Legacy data": every legacy artefact gets
/// `access_basis = 'manual'` — no machine vouched for these bytes.
const ACCESS_BASIS_MANUAL: &str = "manual";

/// A legacy file has no recorded version, so we record `unknown` rather
/// than claim `vor`.
const VERSION_UNKNOWN: &str = "unknown";

/// Blob store layout, ADR-007 §1 "Storage".
const BLOB_DIR: &str = "blobs";
const BLOB_TMP_DIR: &str = ".tmp";

/// The ADR's `locator` for a full-text artefact: `''`.
const FULLTEXT_LOCATOR: &str = "";

/// Copy legacy `papers.local_path` files into the blob store and record
/// them as `artefacts` + `blobs` under `library_root` (the absolute
/// parent directory of the DB file).
///
/// Idempotent: safe to call on every `migrate()`. Never moves or deletes
/// a legacy file. Never fails because of a file — a path that is gone or
/// unreadable is recorded with `missing_on_disk = 1` and `sha256 = NULL`,
/// because a missing legacy file is a fact worth recording rather than a
/// reason to skip the work.
///
/// # Errors
///
/// Returns `DbError` only for SQLite failures (the scan, or the insert
/// transaction). File-system trouble is absorbed into the artefact row it
/// concerns, so a single unreadable paper can never block a startup.
pub fn backfill_legacy_artefacts(
    conn: &mut Connection,
    library_root: &Path,
) -> Result<(), DbError> {
    let now = Utc::now();
    let legacy = scan_legacy_papers(conn)?;
    if legacy.is_empty() {
        return Ok(());
    }

    // Phase 1 — file work with no write lock held, so the transaction
    // below is pure SQL and stays short.
    let mut plans = Vec::with_capacity(legacy.len());
    let mut skipped = 0_usize;
    for row in &legacy {
        match plan_legacy_artefact(row, library_root, now) {
            Some(plan) => plans.push(plan),
            None => skipped += 1,
        }
    }

    // Phase 2 — `blobs` before `artefacts`: `artefacts.sha256` is a
    // foreign key into it.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for plan in &plans {
        if let Some(blob) = &plan.blob {
            tx.execute(
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
        tx.execute(
            "INSERT OR IGNORE INTO artefacts
                 (id, paper_id, kind, version, locator, sha256, format, access_status,
                  route, access_basis, imported_from, retrieved_at, missing_on_disk)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                plan.id,
                plan.paper_id,
                plan.kind,
                VERSION_UNKNOWN,
                FULLTEXT_LOCATOR,
                plan.blob.as_ref().map(|b| b.sha256.as_str()),
                plan.format,
                plan.access_status,
                ROUTE_LEGACY,
                ACCESS_BASIS_MANUAL,
                plan.imported_from,
                plan.retrieved_at,
                i64::from(plan.missing_on_disk),
            ],
        )?;
    }
    tx.commit()?;

    let distinct_blobs = plans
        .iter()
        .filter_map(|p| p.blob.as_ref().map(|b| b.sha256.as_str()))
        .collect::<HashSet<_>>()
        .len();
    let missing = plans.iter().filter(|p| p.missing_on_disk).count();
    tracing::debug!(
        legacy_papers = legacy.len(),
        artefacts = plans.len(),
        blobs = distinct_blobs,
        missing_on_disk = missing,
        skipped_unrecognised_kind = skipped,
        "legacy acquisition backfill complete"
    );
    Ok(())
}

impl Database {
    /// Resolve the library root from the open connection and run
    /// [`backfill_legacy_artefacts`]. Called from `migrate()` next to
    /// `backfill_bibtex_keys`, so an upgrade needs no separate command.
    pub(super) fn backfill_acquisition_artefacts(&self) -> Result<(), DbError> {
        let mut conn = self.pool.get()?;
        match library_root(&conn)? {
            // ADR-007 §1 "Storage": an in-memory database has no root,
            // so it gets no blobs and skips file backfills.
            None => {
                tracing::debug!(
                    "in-memory database has no library root; skipping artefact backfill"
                );
                Ok(())
            }
            Some(root) => backfill_legacy_artefacts(&mut conn, &root),
        }
    }
}

/// The library root: the absolute parent directory of the DB file.
/// `None` for an in-memory database, whose `PRAGMA database_list` file
/// is the empty string.
fn library_root(conn: &Connection) -> Result<Option<PathBuf>, DbError> {
    let file: Option<String> = conn
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .ok();
    let Some(file) = file.filter(|f| !f.is_empty()) else {
        return Ok(None);
    };
    let absolute = std::fs::canonicalize(&file).unwrap_or_else(|_| PathBuf::from(&file));
    Ok(absolute.parent().map(Path::to_path_buf))
}

/// Read every paper that has a legacy file recorded, with only the
/// columns the backfill touches — the full `row_to_paper` deserialize
/// would pull `abstract`, `full_text` and the rest for nothing.
///
/// The predicate is deliberately plain (`IS NOT NULL AND <> ''`, no
/// `TRIM`) so SQLite can serve it from the partial index added in
/// migration 013; a whitespace-only path is dropped in
/// [`plan_legacy_artefact`] instead.
fn scan_legacy_papers(conn: &Connection) -> Result<Vec<LegacyRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, local_path, download_status, last_attempt_at, created_at
         FROM papers
         WHERE local_path IS NOT NULL AND local_path <> ''",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LegacyRow {
                paper_id: r.get(0)?,
                local_path: r.get(1)?,
                download_status: r.get(2)?,
                last_attempt_at: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// One legacy paper, as read by [`scan_legacy_papers`].
struct LegacyRow {
    paper_id: String,
    local_path: String,
    download_status: Option<String>,
    last_attempt_at: Option<String>,
    created_at: String,
}

/// A `blobs` row to `INSERT OR IGNORE`.
struct BlobRow {
    sha256: String,
    bytes: i64,
    mime: &'static str,
    rel_path: String,
    created_at: String,
}

/// An `artefacts` row to `INSERT OR IGNORE`. Its blob, when present, is
/// written first — `artefacts.sha256` references it.
struct ArtefactPlan {
    id: String,
    paper_id: String,
    kind: &'static str,
    format: &'static str,
    access_status: &'static str,
    imported_from: String,
    retrieved_at: String,
    blob: Option<BlobRow>,
    missing_on_disk: bool,
}

/// Plan the artefact for one legacy paper, hashing and copying its file
/// into the blob store.
///
/// `None` means "no artefact": a blank path, or an extension outside the
/// `artefacts.kind` vocabulary (logged, never guessed at).
fn plan_legacy_artefact(
    row: &LegacyRow,
    library_root: &Path,
    now: DateTime<Utc>,
) -> Option<ArtefactPlan> {
    let recorded = row.local_path.trim();
    if recorded.is_empty() {
        return None;
    }

    let ext = extension(recorded);
    let Some((kind, format, mime)) = classify_extension(&ext) else {
        tracing::warn!(
            paper_id = %row.paper_id,
            path = %recorded,
            "legacy file extension is not a full-text kind; no artefact recorded (file left in place)"
        );
        return None;
    };

    // Legacy `local_path` was written as an absolute path; a relative one
    // (hand-edited, or opened through a relative DB path) resolves
    // against the library root, which is how ADR-007 reads every stored
    // path.
    let source = Path::new(recorded);
    let absolute = if source.is_absolute() {
        source.to_path_buf()
    } else {
        library_root.join(source)
    };

    // (blob row, missing_on_disk). `missing_on_disk` means "the path is
    // recorded but the bytes are not in the store under our control" —
    // true for a vanished file and for one we could hash but not copy.
    let (blob, missing_on_disk) = match hash_file(&absolute) {
        Ok((sha256, bytes)) => {
            let rel_path = blob_rel_path(&sha256, &ext);
            let blob = BlobRow {
                sha256,
                bytes,
                mime,
                rel_path,
                created_at: now.to_rfc3339(),
            };
            match store_blob(&absolute, &blob.sha256, &ext, library_root) {
                // Copied into the content-addressed store.
                Ok(()) => (Some(blob), false),
                Err(e) => {
                    // The content is identified but not in the store. Keep
                    // the row (it names where the blob belongs, and every
                    // later run retries the copy), and flag it. The legacy
                    // file itself is untouched.
                    tracing::warn!(
                        paper_id = %row.paper_id,
                        path = %absolute.display(),
                        error = %e,
                        "hashed a legacy file but could not copy it into the blob store"
                    );
                    (Some(blob), true)
                }
            }
        }
        Err(e) => {
            // Gone, or unreadable. Either way we hold no usable bytes, so
            // the artefact records the path without a hash.
            tracing::warn!(
                paper_id = %row.paper_id,
                path = %absolute.display(),
                error = %e,
                "legacy file is not readable; recorded as missing_on_disk"
            );
            (None, true)
        }
    };

    Some(ArtefactPlan {
        id: artefact_id(&row.paper_id, kind, VERSION_UNKNOWN, FULLTEXT_LOCATOR),
        paper_id: row.paper_id.clone(),
        kind,
        format,
        access_status: access_status_for(row.download_status.as_deref()),
        imported_from: absolute.to_string_lossy().into_owned(),
        retrieved_at: retrieved_at(row, now),
        blob,
        missing_on_disk,
    })
}

/// `(kind, format, mime)` for a legacy file extension, or `None` when
/// that extension is not a full-text kind. See the module table.
fn classify_extension(ext: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match ext {
        "pdf" => Some(("fulltext_pdf", "pdf", "application/pdf")),
        "html" | "htm" | "xhtml" => Some(("fulltext_html", "html", "text/html")),
        "nxml" | "jats" => Some(("fulltext_xml", "jats", "application/xml")),
        "xml" => Some(("fulltext_xml", "xml", "application/xml")),
        _ => None,
    }
}

/// The lowercased extension of `path`, without the dot. Non-alphanumeric
/// bytes are dropped so a hostile `local_path` cannot escape the blob
/// store directory via the name it is stored under.
fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|ext| {
            ext.to_string_lossy()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_lowercase()
        })
        .unwrap_or_default()
}

/// Conservative mapping from the legacy `download_status` to
/// `artefacts.access_status`. See the module table; `downloaded` is the
/// only value that vouches for full content.
fn access_status_for(download_status: Option<&str>) -> &'static str {
    match download_status {
        Some("downloaded") => "full_text",
        Some("paywall") => "paywall",
        // `failed` (and any value a future schema might add): a later
        // attempt overwrote the status without clearing what we know
        // about the file, so claim nothing.
        Some(_) | None => "unknown",
    }
}

/// `retrieved_at`: `last_attempt_at` when present, else `created_at`,
/// else now — normalised to RFC 3339 UTC, because every timestamp in
/// migration 013 is RFC 3339 UTC TEXT and the legacy columns are not
/// guaranteed to agree (SQLite's `datetime('now')` writes
/// `YYYY-MM-DD HH:MM:SS`, which is accepted and read as UTC).
fn retrieved_at(row: &LegacyRow, now: DateTime<Utc>) -> String {
    row.last_attempt_at
        .as_deref()
        .into_iter()
        .chain(std::iter::once(row.created_at.as_str()))
        .find_map(parse_legacy_timestamp)
        .map_or_else(|| now.to_rfc3339(), |dt| dt.to_rfc3339())
}

/// Parse either legacy timestamp shape into UTC. Unparseable values fall
/// through to the next candidate rather than poisoning the column.
fn parse_legacy_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// SHA-256 (hex) and byte length of a file, read in chunks so a large
/// legacy PDF never has to fit in memory.
fn hash_file(path: &Path) -> Result<(String, i64), DbError> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| io_error("open", path, &e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    let mut bytes = 0_i64;
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| io_error("read", path, &e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        bytes += read as i64;
    }
    Ok((hex_digest(hasher.finalize().as_slice()), bytes))
}

/// Copy `src` into `<library_root>/blobs/<first 2 hex>/<sha256>.<ext>`
/// and return that store-relative path. The copy is staged in
/// `blobs/.tmp/` — same filesystem, so the rename into place is atomic —
/// and skipped entirely when the destination already exists, which is
/// what makes a re-run cheap.
fn store_blob(src: &Path, sha256: &str, ext: &str, library_root: &Path) -> Result<(), DbError> {
    let dest = library_root.join(blob_rel_path(sha256, ext));
    if dest.exists() {
        // Content-addressed: same digest, same bytes.
        return Ok(());
    }

    let store = library_root.join(BLOB_DIR);
    let staging = store
        .join(BLOB_TMP_DIR)
        .join(format!("{sha256}.{}.tmp", std::process::id()));
    if let Some(dir) = staging.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_error("create blob tmp dir", dir, &e))?;
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_error("create blob dir", dir, &e))?;
    }
    std::fs::copy(src, &staging)
        .map_err(|e| io_error("copy legacy file into blob store", src, &e))?;

    match std::fs::rename(&staging, &dest) {
        Ok(()) => Ok(()),
        // Another process may have won the race with identical bytes.
        Err(_) if dest.exists() => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&staging);
            Err(io_error("place blob in store", &dest, &e))
        }
    }
}

/// Where a blob lives, relative to the library root — ADR-007 §1
/// "Storage": `<root>/blobs/<first 2 hex>/<sha256>.<ext>`.
fn blob_rel_path(sha256: &str, ext: &str) -> String {
    if ext.is_empty() {
        format!("{BLOB_DIR}/{}/{sha256}", &sha256[..2])
    } else {
        format!("{BLOB_DIR}/{}/{sha256}.{ext}", &sha256[..2])
    }
}

/// Deterministic artefact id: the first 16 bytes of SHA-256 over a
/// length-framed `paper_id | kind | version | locator`, rendered as 32
/// lowercase hex — the same id shape the workspace already uses for
/// papers, questions and searches.
///
/// Deterministic rather than a fresh `uuid::Uuid::new_v4()` because
/// idempotence should not depend on the UNIQUE index alone: the same
/// legacy row must derive the same id on every run and on every machine,
/// so a re-run after an interrupted transaction cannot leave a row whose
/// id differs from the one a later run would compute. The domain prefix
/// keeps these digests from ever colliding with another sha256-derived
/// hex id in the database, and the length framing keeps `("ab", "c")`
/// from colliding with `("a", "bc")`.
fn artefact_id(paper_id: &str, kind: &str, version: &str, locator: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"scitadel/artefact-id/v1");
    for part in [paper_id, kind, version, locator] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex_digest(&hasher.finalize()[..16])
}

/// Lowercase hex of `bytes`.
fn hex_digest(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// File-system trouble as a `DbError::Migration`, naming the path that
/// caused it.
fn io_error(what: &str, path: &Path, e: &std::io::Error) -> DbError {
    DbError::Migration(format!("{what} {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A fixed RFC 3339 creation stamp, so the `retrieved_at` fallback
    /// chain is assertable instead of "whatever now was".
    const CREATED_AT: &str = "2026-01-02T03:04:05+00:00";
    const ATTEMPTED_AT: &str = "2026-02-03T04:05:06+00:00";

    /// A migrated file-backed database plus its library root (the temp
    /// dir that holds both `scitadel.db` and the legacy files).
    struct Fixture {
        _dir: TempDir,
        db: Database,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().to_path_buf();
            let db = Database::open(&root.join("scitadel.db")).unwrap();
            db.migrate().unwrap();
            Self {
                _dir: dir,
                db,
                root,
            }
        }

        /// Write a legacy file into the library (under `papers/`, as
        /// `download_paper` did) and return its absolute path.
        fn file(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.root.join("papers").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            path.to_string_lossy().into_owned()
        }

        /// Insert a paper with legacy download state. `local_path` /
        /// `download_status` / `last_attempt_at` are written exactly as
        /// the pre-013 code wrote them.
        fn paper(
            &self,
            id: &str,
            local_path: Option<&str>,
            download_status: Option<&str>,
            last_attempt_at: Option<&str>,
        ) {
            let conn = self.db.conn().unwrap();
            conn.execute(
                "INSERT INTO papers
                     (id, title, authors, created_at, updated_at, local_path, download_status, last_attempt_at)
                 VALUES (?1, ?2, '[]', ?3, ?3, ?4, ?5, ?6)",
                params![id, format!("Legacy paper {id}"), CREATED_AT, local_path, download_status, last_attempt_at],
            )
            .unwrap();
        }

        /// Run the backfill the way `migrate()` does.
        fn backfill(&self) {
            self.db.backfill_acquisition_artefacts().unwrap();
        }

        /// All artefact rows, ordered so a before/after comparison is
        /// stable.
        fn artefacts(&self) -> Vec<Artefact> {
            artefacts_of(&self.db)
        }

        fn blobs(&self) -> Vec<(String, i64, Option<String>, String)> {
            self.blob_rows()
        }

        /// The single `blobs` row as `(sha256, bytes, mime, rel_path)`.
        fn blob_row(&self) -> Option<(String, i64, Option<String>, String)> {
            let rows = self.blob_rows();
            assert_eq!(rows.len(), 1, "expected exactly one blobs row: {rows:?}");
            rows.into_iter().next()
        }

        fn blob_rows(&self) -> Vec<(String, i64, Option<String>, String)> {
            let conn = self.db.conn().unwrap();
            let mut stmt = conn
                .prepare("SELECT sha256, bytes, mime, rel_path FROM blobs ORDER BY sha256")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        }

        fn count(&self, sql: &str) -> i64 {
            let conn = self.db.conn().unwrap();
            conn.query_row(sql, [], |r| r.get(0)).unwrap()
        }
    }

    /// A row of `artefacts` as the tests want to see it.
    #[derive(Debug, PartialEq, Eq)]
    struct Artefact {
        id: String,
        paper_id: String,
        kind: String,
        version: String,
        locator: String,
        sha256: Option<String>,
        format: Option<String>,
        access_status: String,
        route: String,
        access_basis: String,
        imported_from: Option<String>,
        retrieved_at: String,
        missing_on_disk: i64,
    }

    fn artefacts_of(db: &Database) -> Vec<Artefact> {
        let conn = db.conn().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, paper_id, kind, version, locator, sha256, format, access_status,
                        route, access_basis, imported_from, retrieved_at, missing_on_disk
                 FROM artefacts ORDER BY paper_id, kind",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok(Artefact {
                id: r.get(0)?,
                paper_id: r.get(1)?,
                kind: r.get(2)?,
                version: r.get(3)?,
                locator: r.get(4)?,
                sha256: r.get(5)?,
                format: r.get(6)?,
                access_status: r.get(7)?,
                route: r.get(8)?,
                access_basis: r.get(9)?,
                imported_from: r.get(10)?,
                retrieved_at: r.get(11)?,
                missing_on_disk: r.get(12)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    fn artefact_for<'a>(rows: &'a [Artefact], paper_id: &str) -> &'a Artefact {
        rows.iter()
            .find(|a| a.paper_id == paper_id)
            .unwrap_or_else(|| panic!("no artefact for {paper_id} in {rows:?}"))
    }

    /// Distinct PDF/HTML bytes, so two files never collide by accident in
    /// a test that is not about dedup.
    const PDF_BYTES: &[u8] = b"%PDF-1.7\nlegacy paper body\n%%EOF\n";
    const HTML_BYTES: &[u8] = b"<!doctype html><html><body>legacy abstract</body></html>";

    /// ADR-007 §1 "Legacy data": one artefact per legacy file, with the
    /// kind inferred from the extension, `version = 'unknown'`,
    /// `route = 'legacy'`, `access_basis = 'manual'`, and an
    /// `access_status` derived from the legacy `download_status`.
    #[test]
    fn backfill_creates_one_artefact_per_legacy_file() {
        let fx = Fixture::new();
        let pdf = fx.file("alpha.pdf", PDF_BYTES);
        let html = fx.file("beta.html", HTML_BYTES);
        let stub = fx.file("gamma.html", HTML_BYTES);
        fx.paper("p-1", Some(&pdf), Some("downloaded"), Some(ATTEMPTED_AT));
        fx.paper("p-2", Some(&html), Some("downloaded"), Some(ATTEMPTED_AT));
        // Paywalled: the file is on disk, but it is an abstract / stub.
        fx.paper("p-3", Some(&stub), Some("paywall"), None);
        // A hand-recorded path with no recorded outcome at all.
        fx.paper("p-4", Some(&pdf), None, None);

        fx.backfill();

        let rows = fx.artefacts();
        assert_eq!(rows.len(), 4, "one artefact per legacy file: {rows:?}");

        let p1 = artefact_for(&rows, "p-1");
        assert_eq!(p1.kind, "fulltext_pdf");
        assert_eq!(p1.format.as_deref(), Some("pdf"));
        assert_eq!(p1.access_status, "full_text");
        assert_eq!(p1.version, "unknown", "a legacy file has no version");
        assert_eq!(p1.route, "legacy");
        assert_eq!(p1.access_basis, "manual");
        assert_eq!(p1.locator, "", "full text carries no locator");
        assert_eq!(p1.missing_on_disk, 0);
        assert_eq!(p1.imported_from.as_deref(), Some(pdf.as_str()));
        assert_eq!(
            p1.retrieved_at, ATTEMPTED_AT,
            "retrieved_at comes from last_attempt_at"
        );
        assert!(p1.sha256.is_some(), "an existing file is hashed");

        let p2 = artefact_for(&rows, "p-2");
        assert_eq!(p2.kind, "fulltext_html");
        assert_eq!(p2.format.as_deref(), Some("html"));
        assert_eq!(p2.access_status, "full_text");

        let p3 = artefact_for(&rows, "p-3");
        assert_eq!(
            p3.access_status, "paywall",
            "a paywalled file is not the full text"
        );

        let p4 = artefact_for(&rows, "p-4");
        assert_eq!(
            p4.access_status, "unknown",
            "no recorded outcome makes no claim"
        );
        assert_eq!(
            p4.retrieved_at, CREATED_AT,
            "no last_attempt_at falls back to created_at"
        );

        // The file was copied into the content-addressed store, not moved.
        let sha = p1.sha256.clone().unwrap();
        let stored = fx
            .root
            .join("blobs")
            .join(&sha[..2])
            .join(format!("{sha}.pdf"));
        assert_eq!(
            std::fs::read(&stored).unwrap(),
            PDF_BYTES,
            "legacy file is copied into blobs/ and left where it was"
        );
        assert!(Path::new(&pdf).exists(), "the legacy file is never moved");
        assert_eq!(
            stored.parent().unwrap().join("..").canonicalize().unwrap(),
            fx.root.join("blobs").canonicalize().unwrap()
        );
    }

    /// ADR-007's acceptance criterion: running the backfill twice
    /// changes nothing. Row-for-row identical artefacts, no duplicate
    /// blobs, and the same ids.
    #[test]
    fn backfill_is_idempotent() {
        let fx = Fixture::new();
        let pdf = fx.file("alpha.pdf", PDF_BYTES);
        let html = fx.file("beta.html", HTML_BYTES);
        let gone = fx
            .root
            .join("papers/vanished.pdf")
            .to_string_lossy()
            .into_owned();
        fx.paper("p-1", Some(&pdf), Some("downloaded"), Some(ATTEMPTED_AT));
        fx.paper("p-2", Some(&html), Some("paywall"), None);
        fx.paper("p-3", Some(&gone), Some("downloaded"), None);

        fx.backfill();
        let first = fx.artefacts();
        let blobs_first = fx.blobs();
        assert_eq!(first.len(), 3, "all three legacy rows are recorded");

        // Second run: explicit call, then again through `migrate()`.
        fx.backfill();
        fx.db.migrate().unwrap();

        assert_eq!(
            fx.artefacts(),
            first,
            "a second backfill changes nothing at all"
        );
        assert_eq!(fx.blobs(), blobs_first, "no duplicate blobs appear");
        assert_eq!(fx.count("SELECT COUNT(*) FROM artefacts"), 3);
        assert_eq!(
            fx.count("SELECT COUNT(*) FROM blobs"),
            2,
            "one blob per distinct file"
        );

        // The UNIQUE key is what makes it a no-op: the same
        // (paper_id, kind, version, locator) is ignored, not duplicated.
        let duplicate: i64 = fx.count(
            "SELECT COUNT(*) FROM artefacts WHERE paper_id = 'p-1' AND kind = 'fulltext_pdf'",
        );
        assert_eq!(duplicate, 1);
    }

    /// Ids are derived, not generated, so the backfill is a
    /// deterministic function of the legacy state: the same library
    /// contents in a *different* database derive the same artefact ids
    /// and the same hashes. (The recorded `imported_from` absolute path
    /// legitimately differs — that is the library it was in.) A random
    /// v4 id would satisfy `backfill_is_idempotent`, because the UNIQUE
    /// key ignores the retry, while making every rebuilt library a
    /// different artefact table.
    #[test]
    fn the_backfill_is_deterministic_across_databases() {
        let build = || {
            let fx = Fixture::new();
            let pdf = fx.file("alpha.pdf", PDF_BYTES);
            fx.paper("p-1", Some(&pdf), Some("downloaded"), Some(ATTEMPTED_AT));
            fx.backfill();
            let identity = fx
                .artefacts()
                .into_iter()
                .map(|a| (a.id, a.paper_id, a.kind, a.sha256, a.access_status))
                .collect::<Vec<_>>();
            (identity, fx.blobs())
        };
        let (rows_one, blobs_one) = build();
        let (rows_two, blobs_two) = build();
        assert_eq!(rows_one, rows_two, "same legacy state, same derived rows");
        assert_eq!(blobs_one, blobs_two);
        assert_eq!(rows_one.len(), 1);
    }

    /// A recorded path that no longer exists is a fact worth recording:
    /// the artefact stays, with no hash and `missing_on_disk = 1`.
    #[test]
    fn a_missing_legacy_file_is_recorded_not_skipped() {
        let fx = Fixture::new();
        let gone = fx
            .root
            .join("papers/gone.pdf")
            .to_string_lossy()
            .into_owned();
        assert!(!Path::new(&gone).exists());
        fx.paper("p-1", Some(&gone), Some("downloaded"), Some(ATTEMPTED_AT));

        fx.backfill();

        let rows = fx.artefacts();
        assert_eq!(rows.len(), 1, "the missing file is not skipped: {rows:?}");
        let a = artefact_for(&rows, "p-1");
        assert_eq!(a.sha256, None, "no bytes means no hash");
        assert_eq!(a.missing_on_disk, 1);
        assert_eq!(a.kind, "fulltext_pdf");
        assert_eq!(a.route, "legacy");
        assert_eq!(a.version, "unknown");
        assert_eq!(
            a.imported_from.as_deref(),
            Some(gone.as_str()),
            "where the file was is still recorded"
        );
        assert_eq!(
            fx.count("SELECT COUNT(*) FROM blobs"),
            0,
            "no blob row for a file with no bytes"
        );
        assert_eq!(a.access_status, "full_text");
    }

    /// A file we can read but cannot copy (here: `blobs/` is a regular
    /// file, so the store cannot be created) keeps its hash and its
    /// intended store-relative path, and is flagged — an unplaceable
    /// blob is a fact to surface, not a reason to lose the row.
    #[test]
    fn a_file_that_cannot_be_copied_into_the_store_is_still_recorded() {
        let fx = Fixture::new();
        let pdf = fx.file("alpha.pdf", PDF_BYTES);
        fx.paper("p-1", Some(&pdf), Some("downloaded"), None);
        // Sabotage the store: a file where `blobs/` needs to be.
        std::fs::write(fx.root.join("blobs"), b"not a directory").unwrap();

        fx.backfill();

        let rows = fx.artefacts();
        assert_eq!(rows.len(), 1, "the row is kept: {rows:?}");
        let a = artefact_for(&rows, "p-1");
        assert!(
            a.sha256.is_some(),
            "the bytes were readable, so we know the hash"
        );
        assert_eq!(a.missing_on_disk, 1, "but the bytes are not in the store");
        assert!(Path::new(&pdf).exists(), "and the legacy file is untouched");
        let (sha, _, _, rel_path) = fx
            .blob_row()
            .expect("blob row: the hash was computed, so the row is written");
        assert_eq!(
            rel_path,
            format!("blobs/{}/{sha}.pdf", &sha[..2]),
            "stored paths stay relative to the library root even when unplaced"
        );
        assert!(!rel_path.starts_with('/'));
    }

    /// Two works holding identical bytes share one `blobs` row — that is
    /// the whole point of keying blobs by content.
    #[test]
    fn blobs_are_content_addressed_and_deduplicated() {
        let fx = Fixture::new();
        let one = fx.file("one.pdf", PDF_BYTES);
        let two = fx.file("two.pdf", PDF_BYTES);
        assert_ne!(one, two);
        fx.paper("p-1", Some(&one), Some("downloaded"), None);
        fx.paper("p-2", Some(&two), Some("downloaded"), None);

        fx.backfill();

        let rows = fx.artefacts();
        assert_eq!(rows.len(), 2);
        let a = artefact_for(&rows, "p-1");
        let b = artefact_for(&rows, "p-2");
        let sha = a.sha256.clone().expect("hashed");
        assert_eq!(
            b.sha256.as_deref(),
            Some(sha.as_str()),
            "identical bytes, one hash"
        );

        let blobs = fx.blobs();
        assert_eq!(blobs.len(), 1, "one blobs row: {blobs:?}");
        let (hash, bytes, mime, rel_path) = blobs[0].clone();
        assert_eq!(hash, sha);
        assert_eq!(bytes, PDF_BYTES.len() as i64);
        assert_eq!(mime.as_deref(), Some("application/pdf"));
        assert_eq!(rel_path, format!("blobs/{}/{sha}.pdf", &sha[..2]));
        assert_eq!(
            std::fs::read(fx.root.join(&rel_path)).unwrap(),
            PDF_BYTES,
            "rel_path resolves against the library root to the copied bytes"
        );
        assert!(
            !rel_path.starts_with('/'),
            "stored paths are relative, so a moved library still resolves them"
        );

        // Re-running must not add a second blob for the same content.
        fx.backfill();
        assert_eq!(fx.blobs().len(), 1);
    }

    /// Extension decides kind, so a PDF and an HTML legacy file land as
    /// different kinds — including via the alternative HTML and XML
    /// spellings.
    #[test]
    fn a_pdf_and_an_html_file_map_to_different_kinds() {
        let fx = Fixture::new();
        let pdf = fx.file("a.pdf", PDF_BYTES);
        let html = fx.file("b.html", HTML_BYTES);
        let htm = fx.file("c.htm", b"<html>variant</html>");
        let nxml = fx.file("d.nxml", b"<article>JATS</article>");
        fx.paper("p-1", Some(&pdf), Some("downloaded"), None);
        fx.paper("p-2", Some(&html), Some("downloaded"), None);
        fx.paper("p-3", Some(&htm), Some("downloaded"), None);
        fx.paper("p-4", Some(&nxml), Some("downloaded"), None);

        fx.backfill();

        let rows = fx.artefacts();
        assert_eq!(rows.len(), 4);
        assert_eq!(artefact_for(&rows, "p-1").kind, "fulltext_pdf");
        assert_eq!(artefact_for(&rows, "p-2").kind, "fulltext_html");
        assert_eq!(artefact_for(&rows, "p-3").kind, "fulltext_html", ".htm too");
        assert_eq!(artefact_for(&rows, "p-4").kind, "fulltext_xml");
        assert_eq!(artefact_for(&rows, "p-1").format.as_deref(), Some("pdf"));
        assert_eq!(artefact_for(&rows, "p-2").format.as_deref(), Some("html"));
        assert_eq!(artefact_for(&rows, "p-4").format.as_deref(), Some("jats"));
    }

    /// An extension outside the `artefacts.kind` vocabulary is left
    /// alone rather than mislabelled as a full text — the file stays on
    /// disk, and coverage is not told a lie.
    #[test]
    fn an_unrecognised_extension_is_left_alone_rather_than_mislabelled() {
        let fx = Fixture::new();
        let docx = fx.file("a.docx", b"PK\x03\x04 not really a docx");
        fx.paper("p-1", Some(&docx), Some("downloaded"), None);
        fx.paper("p-2", None, Some("downloaded"), None);

        fx.backfill();

        assert!(fx.artefacts().is_empty(), "no artefact for an unknown kind");
        assert!(Path::new(&docx).exists(), "and the file is untouched");
    }

    /// Papers with no legacy file produce no artefacts: no `local_path`
    /// at all, an empty one, or only the pre-0.4 `full_text` column with
    /// no file behind it.
    #[test]
    fn papers_without_a_legacy_file_produce_no_artefacts() {
        let fx = Fixture::new();
        fx.paper("p-1", None, Some("downloaded"), None);
        fx.paper("p-2", Some(""), Some("paywall"), None);
        fx.paper("p-3", None, None, None);
        fx.paper("p-4", Some("   "), Some("downloaded"), None); // whitespace only
        {
            let conn = fx.db.conn().unwrap();
            conn.execute(
                "UPDATE papers SET full_text = ?1, summary = ?2 WHERE id = 'p-3'",
                params!["extracted text with no file behind it", "a summary"],
            )
            .unwrap();
        }

        fx.backfill();

        assert!(
            fx.artefacts().is_empty(),
            "only a recorded file becomes an artefact: {:?}",
            fx.artefacts()
        );
        assert_eq!(fx.count("SELECT COUNT(*) FROM blobs"), 0);
    }

    /// An in-memory database has no library root, so ADR-007 §1
    /// "Storage" has it skip file backfills entirely.
    #[test]
    fn an_in_memory_database_skips_the_file_backfill() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO papers (id, title, authors, created_at, updated_at, local_path, download_status)
             VALUES ('p-1', 'no root', '[]', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00',
                     '/tmp/whatever.pdf', 'downloaded')",
            [],
        )
        .unwrap();
        drop(conn);

        db.backfill_acquisition_artefacts().unwrap();

        let conn = db.conn().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM artefacts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    /// The upgrade path: `Database::migrate()` runs the backfill, so
    /// nobody has to remember a separate command after upgrading.
    #[test]
    fn backfill_runs_as_part_of_migrate() {
        let fx = Fixture::new();
        let pdf = fx.file("alpha.pdf", PDF_BYTES);
        fx.paper("p-1", Some(&pdf), Some("downloaded"), Some(ATTEMPTED_AT));
        assert!(fx.artefacts().is_empty(), "nothing before migrate()");

        // The only call: no direct backfill invocation.
        fx.db.migrate().unwrap();

        let rows = fx.artefacts();
        assert_eq!(rows.len(), 1, "migrate() backfilled: {rows:?}");
        assert_eq!(artefact_for(&rows, "p-1").kind, "fulltext_pdf");
        assert_eq!(artefact_for(&rows, "p-1").route, "legacy");
        assert_eq!(artefact_for(&rows, "p-1").version, "unknown");
        assert!(artefact_for(&rows, "p-1").sha256.is_some());
    }

    /// The id is a pure function of the UNIQUE key, so the same legacy
    /// row derives the same id on every run and every machine.
    #[test]
    fn artefact_ids_are_deterministic_and_distinct() {
        let id = artefact_id("p-1", "fulltext_pdf", VERSION_UNKNOWN, FULLTEXT_LOCATOR);
        assert_eq!(
            id,
            artefact_id("p-1", "fulltext_pdf", VERSION_UNKNOWN, FULLTEXT_LOCATOR)
        );
        assert_ne!(
            id,
            artefact_id("p-2", "fulltext_pdf", VERSION_UNKNOWN, FULLTEXT_LOCATOR)
        );
        assert_ne!(
            id,
            artefact_id("p-1", "fulltext_html", VERSION_UNKNOWN, FULLTEXT_LOCATOR)
        );
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

    /// The library root is the absolute parent of the DB file; an
    /// in-memory DB has none.
    #[test]
    fn library_root_is_the_dbs_parent_directory() {
        let fx = Fixture::new();
        let conn = fx.db.conn().unwrap();
        assert_eq!(
            library_root(&conn).unwrap(),
            Some(fx.root.canonicalize().unwrap())
        );
        drop(conn);

        let mem = Database::open_in_memory().unwrap();
        assert_eq!(library_root(&mem.conn().unwrap()).unwrap(), None);
    }
}
