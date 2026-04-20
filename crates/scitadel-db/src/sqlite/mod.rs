mod annotations;
mod assessments;
mod citations;
mod migrations;
mod paper_state;
mod papers;
mod questions;
mod searches;

pub use annotations::{SqliteAnnotationRepository, resolve_anchor};
pub use assessments::SqliteAssessmentRepository;
pub use citations::SqliteCitationRepository;
pub use migrations::run_migrations;
pub use paper_state::{PaperState, SqlitePaperStateRepository};
pub use papers::SqlitePaperRepository;
pub use questions::SqliteQuestionRepository;
pub use searches::SqliteSearchRepository;

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

    /// Run all pending migrations.
    pub fn migrate(&self) -> Result<(), DbError> {
        let conn = self.pool.get()?;
        run_migrations(&conn)
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
