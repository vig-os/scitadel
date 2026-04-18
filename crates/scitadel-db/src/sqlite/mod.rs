mod assessments;
mod citations;
mod migrations;
mod papers;
mod questions;
mod searches;

pub use assessments::SqliteAssessmentRepository;
pub use citations::SqliteCitationRepository;
pub use migrations::run_migrations;
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
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
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
