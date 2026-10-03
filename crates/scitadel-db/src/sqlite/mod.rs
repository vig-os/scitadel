mod acquisition;
mod annotations;
mod artefacts;
mod assessments;
mod blobs;
mod citations;
mod migrations;
mod pacer;
mod paper_aliases;
mod paper_state;
mod paper_tags;
mod papers;
mod questions;
mod searches;
mod shortlist;
mod tui_state;

pub use acquisition::{ROUTE_LEGACY, backfill_legacy_artefacts};
pub use annotations::{SqliteAnnotationRepository, resolve_anchor};

/// ADR-007 §1: the deterministic artefact id and the `artefacts` /
/// `acquisition_state` writers. Shared by the legacy backfill and the
/// flat-layout importer (`scitadel_adapters::import_flat`).
pub use artefacts::{
    ACCESS_BASIS_MANUAL, ArtefactWrite, BlobWrite, DownloadWrite, FULLTEXT_LOCATOR, FulltextKind,
    ROUTE_IMPORT_FLAT, StateWrite, VERSION_UNKNOWN, WriteMode, artefact_id,
    delete_acquisition_states_at, fulltext_kind, has_fulltext_artefact, record_download,
    write_acquisition_states, write_artefacts, write_artefacts_in,
};
pub use assessments::SqliteAssessmentRepository;
/// ADR-007 §1 "Storage": the content-addressed blob store.
pub use blobs::{blob_rel_path, file_extension, hash_file, library_root, store_blob};
pub use citations::SqliteCitationRepository;
pub use migrations::run_migrations;
pub use pacer::{PolicyLookup, SqlitePacer};
pub use paper_aliases::{SOURCE_BIBTEX_IMPORT, SOURCE_REKEY, SqlitePaperAliasRepository};
pub use paper_state::{PaperState, SqlitePaperStateRepository};
pub use paper_tags::{SqlitePaperTagRepository, TAG_SOURCE_BIBTEX_IMPORT};
pub use papers::SqlitePaperRepository;
pub use questions::SqliteQuestionRepository;
/// Re-export of `rusqlite::Transaction` so downstream crates (e.g.
/// `scitadel-mcp`'s bib-import orchestrator in #157) can drive multi-
/// repo transactions without taking a direct rusqlite dependency.
pub use rusqlite::Transaction as SqliteTransaction;
pub use searches::SqliteSearchRepository;
pub use shortlist::SqliteShortlistRepository;
pub use tui_state::{SqliteTuiStateRepository, TuiState};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::functions::FunctionFlags;
use std::path::{Path, PathBuf};

use crate::error::DbError;

/// Register `unicode_lower(text)` — a Unicode-aware lowercase scalar
/// function for SQL queries. SQLite's built-in `LOWER()` is ASCII-only
/// (`Ü` stays `Ü`), so case-insensitive title comparisons miss
/// non-ASCII case variants. Registered on every connection at init
/// time so any pooled handle can use it. (#159)
fn register_unicode_lower(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "unicode_lower",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let s = ctx.get::<Option<String>>(0)?;
            Ok(s.map(|v| v.to_lowercase()))
        },
    )
}

/// Parse an RFC3339 timestamp string, falling back to now on parse errors.
pub(crate) fn parse_rfc3339_or_now(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map_or_else(|_| chrono::Utc::now(), |dt| dt.with_timezone(&chrono::Utc))
}

/// Minimum length below which we refuse to treat a string as a plausible
/// id prefix. `Id::short()` prints 8 chars; anything shorter than 4 is
/// too likely to sweep in the whole table or an unrelated record. Chosen
/// as half of `short()` so callers who typed the first half of what
/// they read still resolve.
pub const MIN_ID_PREFIX_LEN: usize = 4;

/// True when `s` looks like something an MCP / CLI resolver should
/// hand off to the `id GLOB '{s}*'` prefix scan. The DB layer's
/// `find_by_id_prefix` fns and `scitadel_mcp::tools::resolve_id_prefix`
/// both gate on this so raw user input can never sneak GLOB
/// metacharacters into the pattern — an empty string would GLOB to
/// `*` (match everything and silently resolve to any single row in a
/// one-row table), and `*`, `?`, `[`, `]` are GLOB wildcards that
/// would let arbitrary inputs like `question_id="a?"` match unrelated
/// records. Also enforces `MIN_ID_PREFIX_LEN` so a 1-3 char accidental
/// input can't collide with a hex prefix that happens to fit.
///
/// The allowed character set is ASCII alphanumeric plus `-`, which
/// covers every id shape the workspace emits:
/// - 32-char lowercase-hex `Uuid::simple()` for questions / searches
///   / assessments / annotations / (default) papers,
/// - `W\d+` OpenAlex ids that overwrite `paper.id` in `work_to_paper`,
/// - test fixtures like `p-1`, `p-e2e`.
#[must_use]
pub fn is_valid_id_prefix(s: &str) -> bool {
    if s.len() < MIN_ID_PREFIX_LEN {
        return false;
    }
    s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Database connection pool with migration support.
#[derive(Clone)]
pub struct Database {
    pool: Pool<SqliteConnectionManager>,
}

impl Database {
    /// Open (or create) a database at the given path with WAL mode and FK enforcement.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DbError::Migration(format!("failed to create db directory: {e}")))?;
        }

        let manager = SqliteConnectionManager::file(path).with_init(|conn| {
            // synchronous=NORMAL is the recommended pairing with WAL: full
            // sync is unnecessary because WAL replay covers durability,
            // and NORMAL roughly halves write latency. busy_timeout lets
            // the cross-process 2-pane workflow ride through transient
            // writer locks instead of failing reads. (#121)
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                     PRAGMA synchronous=NORMAL;
                     PRAGMA foreign_keys=ON;
                     PRAGMA busy_timeout=5000;",
            )?;
            register_unicode_lower(conn)?;
            Ok(())
        });

        let pool = Pool::builder().max_size(4).build(manager)?;

        Ok(Self { pool })
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, DbError> {
        let manager = SqliteConnectionManager::memory().with_init(|conn| {
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                     PRAGMA foreign_keys=ON;",
            )?;
            register_unicode_lower(conn)?;
            Ok(())
        });

        let pool = Pool::builder().max_size(1).build(manager)?;

        Ok(Self { pool })
    }

    /// Run all pending migrations, then backfill any paper rows that
    /// lack a stable `bibtex_key` (#132) and any legacy
    /// `local_path` files that predate the `artefacts` table (ADR-007
    /// §1 "Legacy data"). Both backfills are idempotent — on every
    /// subsequent call they are no-ops, because every paper already has
    /// a key and every legacy file already has an artefact row keyed by
    /// `UNIQUE (paper_id, kind, version, locator)`. Papers gain keys via
    /// `save`/`save_many` thereafter, keeping this migrate-call the only
    /// place that needs to know about the assignment algorithm.
    ///
    /// The artefact backfill runs after `run_migrations` commits, so an
    /// upgrade from a pre-013 library needs no separate command: hashing
    /// and copying legacy files happens with no write lock held, and the
    /// rows land in one short transaction (see `sqlite::acquisition`).
    pub fn migrate(&self) -> Result<(), DbError> {
        let conn = self.pool.get()?;
        run_migrations(&conn)?;
        drop(conn);
        self.backfill_bibtex_keys()?;
        self.backfill_acquisition_artefacts()?;
        Ok(())
    }

    /// Walk every paper without a `bibtex_key` and assign one via the
    /// Better-BibTeX-style algorithm in
    /// `scitadel_core::bibtex_key::assign_keys`, preserving uniqueness
    /// against already-assigned keys. Called from `migrate` on upgrade
    /// from pre-0.6.0 schemas and as a safety net on every startup.
    fn backfill_bibtex_keys(&self) -> Result<(), DbError> {
        use scitadel_core::bibtex_key::assign_keys;
        use std::collections::HashSet;

        let conn = self.pool.get()?;

        // Already-assigned keys become the `taken` set.
        let mut taken: HashSet<String> = conn
            .prepare("SELECT bibtex_key FROM papers WHERE bibtex_key IS NOT NULL")?
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(Result::ok)
            .collect();

        // Load papers needing a key. Only the minimum columns the
        // algorithm touches — avoids the full row_to_paper deserialize.
        let mut stmt =
            conn.prepare("SELECT id, title, authors, year FROM papers WHERE bibtex_key IS NULL")?;
        let rows: Vec<(String, String, String, Option<i32>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);

        if rows.is_empty() {
            return Ok(());
        }

        // Reconstitute a minimal `Paper` per row — authors is a JSON
        // array; everything else the algorithm needs is in the columns.
        let papers: Vec<scitadel_core::models::Paper> = rows
            .iter()
            .map(|(id, title, authors_json, year)| {
                let mut p = scitadel_core::models::Paper::new(title);
                p.id = scitadel_core::models::PaperId::from(id.as_str());
                p.authors = serde_json::from_str(authors_json).unwrap_or_default();
                p.year = *year;
                p
            })
            .collect();

        let keys = assign_keys(&papers, &mut taken);
        for (paper, key) in papers.iter().zip(keys) {
            conn.execute(
                "UPDATE papers SET bibtex_key = ?1 WHERE id = ?2",
                rusqlite::params![key, paper.id.as_str()],
            )?;
        }
        Ok(())
    }

    /// Get a connection from the pool.
    pub fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, DbError> {
        Ok(self.pool.get()?)
    }

    /// ADR-007 §1 "Storage": the library root this database belongs to,
    /// or `None` for an in-memory database.
    pub fn library_root(&self) -> Result<Option<PathBuf>, DbError> {
        let conn = self.pool.get()?;
        blobs::library_root(&conn)
    }

    /// ADR-007 §1: write `artefacts` rows (and their blobs) in one short
    /// transaction. See [`write_artefacts`] for the conflict handling.
    ///
    /// A method rather than only a free function so callers outside this
    /// crate — the flat-layout importer — never have to take a direct
    /// `rusqlite` dependency to write a row.
    pub fn write_artefacts(
        &self,
        rows: &[ArtefactWrite],
        mode: WriteMode,
    ) -> Result<usize, DbError> {
        let mut conn = self.pool.get()?;
        artefacts::write_artefacts(&mut conn, rows, mode)
    }

    /// ADR-007 §1 "Legacy data": record one completed download, writing
    /// the `artefacts` row and the legacy `papers` columns in a single
    /// transaction. See [`record_download`].
    ///
    /// A method for the same reason as [`Self::write_artefacts`]: the
    /// download chain lives outside this crate and must not need a direct
    /// `rusqlite` dependency to record what it fetched.
    pub fn record_download(&self, write: &DownloadWrite) -> Result<(), DbError> {
        let mut conn = self.pool.get()?;
        artefacts::record_download(&mut conn, write)
    }

    /// ADR-007 §1: record one wanted-but-absent artefact. See
    /// [`write_acquisition_states`].
    pub fn upsert_acquisition_state(&self, row: &StateWrite) -> Result<(), DbError> {
        let mut conn = self.pool.get()?;
        artefacts::write_acquisition_states(&mut conn, std::slice::from_ref(row))?;
        Ok(())
    }

    /// ADR-007 §1 "Have": does this work already hold a full text?
    pub fn has_fulltext_artefact(&self, paper_id: &str) -> Result<bool, DbError> {
        let conn = self.pool.get()?;
        artefacts::has_fulltext_artefact(&conn, paper_id)
    }

    /// ADR-007 §1: retract a gap recorded at `drop_path` — see
    /// [`delete_acquisition_states_at`] for why the scoping is that
    /// narrow.
    pub fn retract_acquisition_gap(
        &self,
        paper_id: &str,
        drop_path: &str,
    ) -> Result<usize, DbError> {
        let conn = self.pool.get()?;
        artefacts::delete_acquisition_states_at(&conn, paper_id, drop_path)
    }

    /// Create all repository instances sharing this database.
    pub fn repositories(
        &self,
    ) -> (
        SqlitePaperRepository,
        SqliteSearchRepository,
        SqliteQuestionRepository,
        SqliteAssessmentRepository,
        SqliteCitationRepository,
    ) {
        let db = self.clone();
        (
            SqlitePaperRepository::new(db.clone()),
            SqliteSearchRepository::new(db.clone()),
            SqliteQuestionRepository::new(db.clone()),
            SqliteAssessmentRepository::new(db.clone()),
            SqliteCitationRepository::new(db),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::Paper;
    use scitadel_core::ports::PaperRepository;

    /// #232 review blocker: raw user input must never sneak GLOB
    /// metacharacters or "match anything" shapes into the prefix
    /// scan. `is_valid_id_prefix` is the single truth-source that
    /// every `find_by_id_prefix` gates on defensively, and that
    /// `scitadel_mcp::tools::resolve_id_prefix` gates on primarily.
    #[test]
    fn is_valid_id_prefix_rejects_empty_and_glob_metachars_and_short() {
        // Empty → would GLOB to `*` and match everything.
        assert!(!is_valid_id_prefix(""));
        // GLOB metacharacters would let hostile inputs sweep unrelated rows.
        for hostile in ["*", "?", "[abc", "abc*", "a?", "a?bc", "abc]", "[a]"] {
            assert!(!is_valid_id_prefix(hostile), "{hostile} must be rejected");
        }
        // Below MIN_ID_PREFIX_LEN → too collision-prone to be a real id.
        for short in ["a", "ab", "abc", "p-1"] {
            assert!(
                !is_valid_id_prefix(short),
                "{short} is shorter than MIN_ID_PREFIX_LEN and must be rejected"
            );
        }
        // Whitespace, quotes, SQL/GLOB adjacent bytes all rejected.
        for junk in ["    ", "  a ", "abc ", "abc'", "abc%", "abc_x"] {
            assert!(!is_valid_id_prefix(junk), "{junk:?} must be rejected");
        }
    }

    /// Every real id shape emitted by the workspace must pass so the
    /// gate never blocks a legitimate copy-pasted id. Enumerates the
    /// three known families: UUID `simple()` hex (questions / searches
    /// / assessments / annotations / default papers), OpenAlex short
    /// ids (`W\d+`) that overwrite `paper.id`, and dashed test
    /// fixtures.
    #[test]
    fn is_valid_id_prefix_accepts_every_real_id_shape() {
        // 4-char short prefix (min length) up to full 32-char UUID.
        for id in [
            "cafe",
            "cafebabe",
            "cafebabe1234",
            "cafebabe12345678abcd1234ef567890",
            "W2741809807",
            "p-e2e",
            "p-1234-x",
        ] {
            assert!(is_valid_id_prefix(id), "{id} should be accepted");
        }
    }

    /// Two `Database` instances pointing at the same file simulate the
    /// 2-pane workflow: TUI process holds one, `scitadel mcp` process
    /// holds the other. A write through one must surface through the
    /// other — that's the contract WAL mode buys us. (#121)
    #[test]
    fn cross_process_write_visible_within_one_redraw() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("scitadel.db");

        // Process A (e.g. the TUI) opens, migrates, holds open.
        let db_a = Database::open(&db_path).unwrap();
        db_a.migrate().unwrap();
        let (paper_repo_a, _, _, _, _) = db_a.repositories();

        // Process B (e.g. scitadel mcp) opens the same file independently.
        let db_b = Database::open(&db_path).unwrap();
        let (paper_repo_b, _, _, _, _) = db_b.repositories();

        // Pre-write sanity: A sees nothing.
        assert!(paper_repo_a.list_all(10, 0).unwrap().is_empty());

        // B writes.
        let p = Paper::new("MCP-side write");
        paper_repo_b.save(&p).unwrap();

        // A must see it on the very next read — no commit barrier, no
        // sleep, no reconnect. This is the property the 2-pane workflow
        // depends on.
        let papers = paper_repo_a.list_all(10, 0).unwrap();
        assert_eq!(papers.len(), 1, "TUI process must see MCP process's write");
        assert_eq!(papers[0].title, "MCP-side write");
    }

    /// Backfill invariant (#132): after `migrate()`, every paper has a
    /// `bibtex_key`, keys are unique, and re-migrating is a no-op.
    #[test]
    fn migrate_backfills_bibtex_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("backfill.db");
        let db = Database::open(&db_path).unwrap();
        db.migrate().unwrap();

        // Seed three papers with distinct metadata, omitting bibtex_key.
        let conn = db.conn().unwrap();
        for (id, title, authors, year) in [
            (
                "p-1",
                "Attention Is All You Need",
                r#"["Vaswani, A."]"#,
                2017,
            ),
            ("p-2", "Deep Residual Learning", r#"["Kaiming He"]"#, 2015),
            ("p-3", "Quantum Computing", r#"["Müller, Hans"]"#, 2023),
        ] {
            conn.execute(
                "INSERT INTO papers (id, title, authors, year, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))",
                rusqlite::params![id, title, authors, year],
            )
            .unwrap();
        }
        // Null out the key column (migrate() would have backfilled above).
        conn.execute("UPDATE papers SET bibtex_key = NULL", [])
            .unwrap();
        drop(conn);

        db.migrate().unwrap();

        let conn = db.conn().unwrap();
        let keys: Vec<String> = conn
            .prepare("SELECT bibtex_key FROM papers ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(keys.len(), 3, "every paper got a key");
        assert!(
            keys.contains(&"vaswani2017attention".to_string())
                || keys.contains(&"vaswani2017transformer".to_string())
                || keys.iter().any(|k| k.starts_with("vaswani2017")),
            "got: {keys:?}"
        );
        // All keys distinct.
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());

        // Re-run is a no-op — keys don't change.
        db.migrate().unwrap();
        let keys2: Vec<String> = db
            .conn()
            .unwrap()
            .prepare("SELECT bibtex_key FROM papers ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(keys, keys2, "re-migrate is idempotent");
    }

    #[test]
    fn pragma_journal_mode_is_wal_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("pragma.db");
        let db = Database::open(&db_path).unwrap();
        let conn = db.conn().unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }
}
