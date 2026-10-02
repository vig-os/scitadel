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
//! *conflict handling*, because the two callers mean different things by
//! "run it again":
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

use std::collections::HashSet;

use rusqlite::params;
use rusqlite::{Connection, TransactionBehavior};
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
    pub hint_url: Option<String>,
    /// Where a human should put the file to close this gap.
    pub drop_path: Option<String>,
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
    let statement = artefacts_insert(mode);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for row in rows {
        if let Some(blob) = &row.blob {
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
                row.imported_from,
                row.retrieved_at,
                i64::from(row.missing_on_disk),
            ],
        )?;
    }
    tx.commit()?;
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
fn artefacts_insert(mode: WriteMode) -> String {
    let values = "(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)";
    let columns = "(id, paper_id, kind, version, locator, sha256, format, access_status,
                   route, access_basis, label, caption, source_url, imported_from,
                   retrieved_at, missing_on_disk)";
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
    }
}

/// Upsert `acquisition_state` rows, returning how many were written.
///
/// Like [`write_artefacts`] in [`WriteMode::Reconcile`], an identical
/// re-run updates nothing — a gap row's `updated_at` should say when the
/// gap last *changed*, not when someone looked at it again.
pub fn write_acquisition_states(
    conn: &mut Connection,
    rows: &[StateWrite],
) -> Result<usize, DbError> {
    if rows.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for row in rows {
        tx.execute(
            "INSERT INTO acquisition_state
                 (paper_id, kind, locator, wanted_version, status, reason, hint_url, drop_path, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (paper_id, kind, locator) DO UPDATE SET
               wanted_version = excluded.wanted_version,
               status = excluded.status,
               reason = excluded.reason,
               hint_url = excluded.hint_url,
               drop_path = excluded.drop_path,
               updated_at = excluded.updated_at
             WHERE acquisition_state.wanted_version IS NOT excluded.wanted_version
                OR acquisition_state.status IS NOT excluded.status
                OR acquisition_state.reason IS NOT excluded.reason
                OR acquisition_state.hint_url IS NOT excluded.hint_url
                OR acquisition_state.drop_path IS NOT excluded.drop_path",
            params![
                row.paper_id,
                row.kind,
                row.locator,
                row.wanted_version,
                row.status,
                row.reason,
                row.hint_url,
                row.drop_path,
                row.updated_at,
            ],
        )?;
    }
    tx.commit()?;
    Ok(rows.len())
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

    fn artefacts_of(db: &Database) -> Vec<(String, String, Option<String>, String)> {
        let conn = db.conn().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT kind, locator, sha256, retrieved_at FROM artefacts
                 ORDER BY kind, locator",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
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
        assert_eq!(rows[0].2.as_deref(), Some("aaa"), "the first write wins");
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
        assert_eq!(after[0].2.as_deref(), Some("bbb"));
        assert_eq!(after[0].1, first[0].1, "locator is the stable key");
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
        assert_eq!(artefacts_of(&db)[0].3, "2027-06-06T06:06:06+00:00");
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
            rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
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
            hint_url: None,
            drop_path: Some("/lib/papers/stem/fulltext.pdf".into()),
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
