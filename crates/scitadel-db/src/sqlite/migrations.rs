use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::error::DbError;

const MIGRATION_001: &str = include_str!("../../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_citations.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_full_text.sql");
const MIGRATION_004: &str = include_str!("../../migrations/004_paper_state.sql");
const MIGRATION_005: &str = include_str!("../../migrations/005_annotations.sql");
const MIGRATION_006: &str = include_str!("../../migrations/006_search_fts.sql");
const MIGRATION_007: &str = include_str!("../../migrations/007_paper_download_state.sql");
const MIGRATION_008: &str = include_str!("../../migrations/008_tui_state.sql");
const MIGRATION_009: &str = include_str!("../../migrations/009_bibtex_keys.sql");
const MIGRATION_010: &str = include_str!("../../migrations/010_shortlists.sql");
const MIGRATION_011: &str = include_str!("../../migrations/011_paper_aliases.sql");
const MIGRATION_012: &str = include_str!("../../migrations/012_paper_tags.sql");
const MIGRATION_013: &str = include_str!("../../migrations/013_acquisition.sql");

const MIGRATIONS: &[(i64, &str)] = &[
    (1, MIGRATION_001),
    (2, MIGRATION_002),
    (3, MIGRATION_003),
    (4, MIGRATION_004),
    (5, MIGRATION_005),
    (6, MIGRATION_006),
    (7, MIGRATION_007),
    (8, MIGRATION_008),
    (9, MIGRATION_009),
    (10, MIGRATION_010),
    (11, MIGRATION_011),
    (12, MIGRATION_012),
    (13, MIGRATION_013),
];

/// Run all pending migrations, skipping already-applied ones.
pub fn run_migrations(conn: &Connection) -> Result<(), DbError> {
    run_migrations_with(conn, MIGRATIONS)
}

/// Minimum time a contending migrator waits for the write lock before
/// giving up. `Database::open` sets 5000ms on pooled connections, but
/// `run_migrations` is also reachable through a bare `Connection`, and a
/// second process starting alongside the first (the TUI + MCP two-pane
/// workflow) must queue rather than fail. Matches the 5000ms used for
/// ordinary cross-process writes in `Database::open`.
const MIGRATION_LOCK_TIMEOUT_MS: i64 = 5000;

/// Apply `migrations` in ascending version order, skipping any already
/// recorded in `schema_version`.
///
/// The whole run happens inside one `BEGIN IMMEDIATE` transaction. That
/// buys two properties a bare `execute_batch` loop does not have, both
/// of which matter because scitadel is routinely opened by two processes
/// against one database file (TUI in one pane, `scitadel mcp` in the
/// next) — see #250:
///
/// 1. **Atomic.** A migration that fails partway rolls back whole, so no
///    half-applied `ALTER TABLE` series is left behind. Without this, one
///    bad statement is unrecoverable: the next start re-runs the migration,
///    trips "duplicate column name", and the database never opens again.
/// 2. **Serialized.** `IMMEDIATE` takes the write lock *before*
///    `schema_version` is read, so the read happens under the lock. A
///    second migrator blocks here until the first commits, then observes
///    the committed versions and skips them — instead of reading an empty
///    table and racing into duplicate-column errors.
///
/// `migrations` is a parameter rather than the `MIGRATIONS` constant so
/// tests can drive a synthetic set; production always uses `MIGRATIONS`.
fn run_migrations_with(conn: &Connection, migrations: &[(i64, &str)]) -> Result<(), DbError> {
    ensure_lock_timeout(conn)?;

    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|e| DbError::Migration(format!("failed to acquire migration lock: {e}")))?;

    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        )",
    )
    .map_err(|e| DbError::Migration(e.to_string()))?;

    let applied: Vec<i64> = {
        let mut stmt = tx
            .prepare("SELECT version FROM schema_version")
            .map_err(|e| DbError::Migration(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| DbError::Migration(e.to_string()))?;
        rows.filter_map(Result::ok).collect()
    };

    for &(version, sql) in migrations {
        if applied.contains(&version) {
            continue;
        }
        tx.execute_batch(sql)
            .map_err(|e| DbError::Migration(format!("migration {version} failed: {e}")))?;

        // Defence in depth: the `.sql` files self-record their own
        // version, but a future migration that forgets would then
        // re-run on every start. Recording here too makes the
        // version-gating contract enforced by construction rather than
        // by convention — `INSERT OR IGNORE` keeps it compatible with
        // the in-file statements.
        tx.execute(
            "INSERT OR IGNORE INTO schema_version (version, applied_at)
             VALUES (?1, datetime('now'))",
            params![version],
        )
        .map_err(|e| DbError::Migration(format!("migration {version} bookkeeping failed: {e}")))?;
    }

    tx.commit()
        .map_err(|e| DbError::Migration(format!("failed to commit migrations: {e}")))
}

/// Raise `busy_timeout` to at least [`MIGRATION_LOCK_TIMEOUT_MS`],
/// leaving a longer timeout a caller already configured untouched.
///
/// Without this, a bare `Connection` (no `Database::open` init closure)
/// races a concurrent migrator and fails immediately with
/// "database is locked" instead of waiting its turn.
fn ensure_lock_timeout(conn: &Connection) -> Result<(), DbError> {
    let current: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .map_err(|e| DbError::Migration(format!("failed to read busy_timeout: {e}")))?;

    if current < MIGRATION_LOCK_TIMEOUT_MS {
        conn.execute_batch(&format!(
            "PRAGMA busy_timeout = {MIGRATION_LOCK_TIMEOUT_MS}"
        ))
        .map_err(|e| DbError::Migration(format!("failed to set busy_timeout: {e}")))?;
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

    /// Every migration must leave exactly one `schema_version` row, so a
    /// later `run_migrations` can skip it. The `.sql` files self-record
    /// via `INSERT OR IGNORE`; this pins that contract so a new
    /// migration that forgets cannot silently re-run on every start.
    #[test]
    fn every_migration_records_its_version() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let recorded: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT version FROM schema_version ORDER BY version")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };

        let expected: Vec<i64> = MIGRATIONS.iter().map(|(v, _)| *v).collect();
        assert_eq!(recorded, expected);
    }

    /// #250: a migration that fails partway must leave *no* trace —
    /// neither the columns it managed to add nor a version row. Without
    /// a transaction, a half-applied `ALTER TABLE` series permanently
    /// wedges the database: the next start re-runs it, trips
    /// "duplicate column name", and can never recover.
    #[test]
    fn failing_migration_rolls_back_completely() {
        let conn = Connection::open_in_memory().unwrap();
        // Migration 1 succeeds, migration 2 dies on its second statement.
        let broken = &[
            (
                1,
                "CREATE TABLE a (x TEXT);\nINSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (1, datetime('now'));",
            ),
            (
                2,
                "ALTER TABLE a ADD COLUMN y TEXT;\nTHIS IS NOT SQL;\nINSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (2, datetime('now'));",
            ),
        ];

        run_migrations_with(&conn, broken).unwrap_err();

        // The first migration is rolled back along with the second.
        let has_a: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_a, 0, "partial schema must not survive a failure");

        // `schema_version` is created inside the same transaction, so a
        // rollback takes it with it. Either it is absent or it is empty —
        // what must never happen is a version row for work that was
        // rolled back, which would make the migration silently skipped
        // on the next start.
        let versions: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='table' AND name='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if versions == 1 {
            let rows: i64 = conn
                .query_row("SELECT count(*) FROM schema_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                rows, 0,
                "no version may be recorded for rolled-back work, or the \
                 migration is skipped forever"
            );
        }

        // The database must still be usable: re-running with the broken
        // migration removed applies everything cleanly.
        run_migrations_with(&conn, &broken[..1]).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    /// #250: two processes starting at once — the documented TUI + MCP
    /// two-pane workflow — must not race.
    ///
    /// Both migrators open the same *fresh* database and start together
    /// on a barrier. The hazard in the pre-#250 code was that each
    /// migration autocommitted on its own: a second migrator could read
    /// an empty `schema_version`, conclude nothing had been applied, and
    /// then re-run `ALTER TABLE papers ADD COLUMN full_text` after the
    /// first had committed it — a hard "duplicate column name" at
    /// startup, unrecoverable without deleting the database. `BEGIN
    /// IMMEDIATE` makes read-and-apply one atomic step, so the loser
    /// simply observes the winner's committed versions and skips them.
    #[test]
    fn two_processes_migrating_a_fresh_db_both_succeed() {
        use std::sync::{Arc, Barrier};

        // Repeated: the race is timing-dependent, so a single pass could
        // pass by luck against a broken implementation.
        for attempt in 0..8 {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().join(format!("race-{attempt}.db"));

            let barrier = Arc::new(Barrier::new(2));
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let path = db_path.clone();
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        let conn = Connection::open(&path).unwrap();
                        barrier.wait();
                        run_migrations(&conn)
                    })
                })
                .collect();

            for handle in handles {
                handle.join().unwrap().unwrap_or_else(|e| {
                    panic!("attempt {attempt}: concurrent migrate failed: {e:?}")
                });
            }

            let conn = Connection::open(&db_path).unwrap();
            let versions: i64 = conn
                .query_row("SELECT count(*) FROM schema_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                versions,
                MIGRATIONS.len() as i64,
                "attempt {attempt}: every version recorded exactly once"
            );
        }
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
        assert!(tables.contains(&"paper_state".to_string()));
        assert!(tables.contains(&"annotations".to_string()));
        assert!(tables.contains(&"annotation_reads".to_string()));
        assert!(tables.contains(&"searches_fts".to_string()));
        assert!(tables.contains(&"paper_aliases".to_string()));
        assert!(tables.contains(&"paper_tags".to_string()));
        // ADR-007 §1: every work artefact, the want-list behind coverage,
        // and the pacer ledger hang off these. A rename here would be
        // invisible to the rest of the suite until acquisition runs.
        assert!(tables.contains(&"blobs".to_string()));
        assert!(tables.contains(&"artefacts".to_string()));
        assert!(tables.contains(&"acquisition_state".to_string()));
        assert!(tables.contains(&"acquisition_attempts".to_string()));
        assert!(tables.contains(&"acquisition_leases".to_string()));
        assert!(tables.contains(&"paper_identity_checks".to_string()));
        assert!(tables.contains(&"pacer_buckets".to_string()));
        assert!(tables.contains(&"pacer_grants".to_string()));
        assert!(tables.contains(&"tdm_authorisations".to_string()));
        assert!(tables.contains(&"schema_version".to_string()));
    }
}
