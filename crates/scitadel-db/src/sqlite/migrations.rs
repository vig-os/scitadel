use rusqlite::Connection;

use crate::error::DbError;

const MIGRATION_001: &str = include_str!("../../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_citations.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_full_text.sql");

const MIGRATIONS: &[(i64, &str)] = &[
    (1, MIGRATION_001),
    (2, MIGRATION_002),
    (3, MIGRATION_003),
];

/// Run all pending migrations, skipping already-applied ones.
pub fn run_migrations(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        )",
    )
    .map_err(|e| DbError::Migration(e.to_string()))?;

    let applied: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT version FROM schema_version")
            .map_err(|e| DbError::Migration(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| DbError::Migration(e.to_string()))?;
        rows.filter_map(Result::ok).collect()
    };

    for &(version, sql) in MIGRATIONS {
        if applied.contains(&version) {
            continue;
        }
        conn.execute_batch(sql)
            .map_err(|e| DbError::Migration(format!("migration {version} failed: {e}")))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // should not fail
    }

    #[test]
    fn test_all_tables_created() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };

        assert!(tables.contains(&"papers".to_string()));
        assert!(tables.contains(&"searches".to_string()));
        assert!(tables.contains(&"search_results".to_string()));
        assert!(tables.contains(&"research_questions".to_string()));
        assert!(tables.contains(&"search_terms".to_string()));
        assert!(tables.contains(&"assessments".to_string()));
        assert!(tables.contains(&"citations".to_string()));
        assert!(tables.contains(&"snowball_runs".to_string()));
        assert!(tables.contains(&"schema_version".to_string()));
    }
}
