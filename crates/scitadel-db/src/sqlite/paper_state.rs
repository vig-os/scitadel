//! Per-reader paper state (star / to-read / read). Thin CRUD over the
//! `paper_state` table; upserts are idempotent on (paper_id, reader).

use rusqlite::params;

use crate::error::DbError;
use crate::sqlite::Database;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaperState {
    pub paper_id: String,
    pub reader: String,
    pub starred: bool,
    pub to_read: bool,
    /// ISO-8601 timestamp when the paper was marked read, or `None` if unread.
    pub read_at: Option<String>,
}

#[derive(Clone)]
pub struct SqlitePaperStateRepository {
    db: Database,
}

impl SqlitePaperStateRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Load the state for one paper, or `None` if nothing is recorded yet.
    pub fn get(&self, paper_id: &str, reader: &str) -> Result<Option<PaperState>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT paper_id, reader, starred, to_read, read_at
             FROM paper_state
             WHERE paper_id = ?1 AND reader = ?2",
        )?;
        let mut rows = stmt.query(params![paper_id, reader])?;
        if let Some(row) = rows.next()? {
            Ok(Some(PaperState {
                paper_id: row.get(0)?,
                reader: row.get(1)?,
                starred: row.get::<_, i64>(2)? != 0,
                to_read: row.get::<_, i64>(3)? != 0,
                read_at: row.get::<_, Option<String>>(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Upsert full state for a (paper, reader) pair.
    pub fn set(&self, state: &PaperState) -> Result<(), DbError> {
        let conn = self.db.conn()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO paper_state (paper_id, reader, starred, to_read, read_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(paper_id, reader) DO UPDATE SET
                 starred    = excluded.starred,
                 to_read    = excluded.to_read,
                 read_at    = excluded.read_at,
                 updated_at = excluded.updated_at",
            params![
                state.paper_id,
                state.reader,
                i64::from(state.starred),
                i64::from(state.to_read),
                state.read_at,
                now,
            ],
        )?;
        Ok(())
    }

    /// Toggle `starred` and return the new value. Creates the row if needed.
    pub fn toggle_starred(&self, paper_id: &str, reader: &str) -> Result<bool, DbError> {
        let existing = self.get(paper_id, reader)?;
        let new_state = match existing {
            Some(mut s) => {
                s.starred = !s.starred;
                s
            }
            None => PaperState {
                paper_id: paper_id.into(),
                reader: reader.into(),
                starred: true,
                to_read: false,
                read_at: None,
            },
        };
        self.set(&new_state)?;
        Ok(new_state.starred)
    }

    /// Load starred paper IDs for one reader as a `HashSet`.
    pub fn starred_ids(&self, reader: &str) -> Result<std::collections::HashSet<String>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt =
            conn.prepare("SELECT paper_id FROM paper_state WHERE reader = ?1 AND starred = 1")?;
        let rows = stmt.query_map(params![reader], |row| row.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new();
        for r in rows {
            out.insert(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        // Need a paper row to satisfy the FK.
        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO papers (id, title, created_at, updated_at)
             VALUES ('p1', 't', datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();
        db
    }

    #[test]
    fn toggle_starred_roundtrip() {
        let db = fresh_db();
        let repo = SqlitePaperStateRepository::new(db);
        assert!(repo.toggle_starred("p1", "lars").unwrap());
        assert!(!repo.toggle_starred("p1", "lars").unwrap());
        assert!(repo.toggle_starred("p1", "lars").unwrap());
        assert!(repo.get("p1", "lars").unwrap().is_some_and(|s| s.starred));
    }

    #[test]
    fn different_readers_have_independent_state() {
        let db = fresh_db();
        let repo = SqlitePaperStateRepository::new(db);
        repo.toggle_starred("p1", "lars").unwrap();
        assert!(!repo.get("p1", "claude").unwrap().is_some_and(|s| s.starred));
        assert!(repo.get("p1", "lars").unwrap().is_some_and(|s| s.starred));
    }

    #[test]
    fn starred_ids_lists_only_starred() {
        let db = fresh_db();
        let repo = SqlitePaperStateRepository::new(db);
        repo.toggle_starred("p1", "me").unwrap();
        let ids = repo.starred_ids("me").unwrap();
        assert!(ids.contains("p1"));
    }
}
