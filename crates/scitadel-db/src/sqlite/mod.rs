mod annotations;
mod assessments;
mod citations;
mod migrations;
mod paper_aliases;
mod paper_state;
mod papers;
mod questions;
mod searches;
mod shortlist;
mod tui_state;

pub use annotations::{SqliteAnnotationRepository, resolve_anchor};
pub use assessments::SqliteAssessmentRepository;
pub use citations::SqliteCitationRepository;
pub use migrations::run_migrations;
pub use paper_aliases::{SOURCE_BIBTEX_IMPORT, SOURCE_REKEY, SqlitePaperAliasRepository};
pub use paper_state::{PaperState, SqlitePaperStateRepository};
pub use papers::SqlitePaperRepository;
pub use questions::SqliteQuestionRepository;
pub use searches::SqliteSearchRepository;
pub use shortlist::SqliteShortlistRepository;
pub use tui_state::{SqliteTuiStateRepository, TuiState};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

use crate::error::DbError;

/// Parse an RFC3339 timestamp string, falling back to now on parse errors.
pub(crate) fn parse_rfc3339_or_now(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map_or_else(|_| chrono::Utc::now(), |dt| dt.with_timezone(&chrono::Utc))
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
            Ok(())
        });

        let pool = Pool::builder().max_size(1).build(manager)?;

        Ok(Self { pool })
    }

    /// Run all pending migrations, then backfill any paper rows that
    /// lack a stable `bibtex_key` (#132). The backfill is idempotent —
    /// on every subsequent call it's a no-op because every paper
    /// already has a key. Papers gain keys via `save`/`save_many`
    /// thereafter, keeping this migrate-call the only place that
    /// needs to know about the assignment algorithm.
    pub fn migrate(&self) -> Result<(), DbError> {
        let conn = self.pool.get()?;
        run_migrations(&conn)?;
        drop(conn);
        self.backfill_bibtex_keys()?;
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
