//! Singleton TUI-selection state (#122). Lets an MCP-side agent read
//! "what is the open scitadel TUI looking at right now?" so it can
//! score the current paper / draft a question from the current
//! context without the user pasting IDs.
//!
//! Last-writer-wins if multiple TUIs run concurrently — acceptable for
//! v1 since the TUI is a single-pane terminal app.

use rusqlite::params;

use crate::error::DbError;
use crate::sqlite::Database;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiState {
    /// Active tab name: "Searches" | "Papers" | "Questions"
    pub tab: String,
    pub paper_id: Option<String>,
    pub search_id: Option<String>,
    pub question_id: Option<String>,
    pub annotation_id: Option<String>,
    /// RFC3339 timestamp of the last TUI-side write.
    pub updated_at: String,
}

#[derive(Clone)]
pub struct SqliteTuiStateRepository {
    db: Database,
}

impl SqliteTuiStateRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Upsert the singleton row. Called by the TUI on every focus /
    /// overlay / tab change. Idempotent.
    pub fn set(&self, state: &TuiState) -> Result<(), DbError> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT INTO tui_state (id, tab, paper_id, search_id, question_id, annotation_id, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 tab           = excluded.tab,
                 paper_id      = excluded.paper_id,
                 search_id     = excluded.search_id,
                 question_id   = excluded.question_id,
                 annotation_id = excluded.annotation_id,
                 updated_at    = excluded.updated_at",
            params![
                state.tab,
                state.paper_id,
                state.search_id,
                state.question_id,
                state.annotation_id,
                state.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Read the singleton row. Returns `None` if no TUI has ever
    /// written (rather than synthesising an empty default).
    pub fn get(&self) -> Result<Option<TuiState>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT tab, paper_id, search_id, question_id, annotation_id, updated_at
             FROM tui_state WHERE id = 1",
        )?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            Ok(Some(TuiState {
                tab: row.get(0)?,
                paper_id: row.get(1)?,
                search_id: row.get(2)?,
                question_id: row.get(3)?,
                annotation_id: row.get(4)?,
                updated_at: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> SqliteTuiStateRepository {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        SqliteTuiStateRepository::new(db)
    }

    #[test]
    fn empty_until_first_write() {
        let repo = fresh();
        assert!(repo.get().unwrap().is_none());
    }

    #[test]
    fn upsert_round_trip() {
        let repo = fresh();
        let s = TuiState {
            tab: "Papers".into(),
            paper_id: Some("p-abc".into()),
            search_id: None,
            question_id: None,
            annotation_id: None,
            updated_at: "2026-04-20T00:00:00Z".into(),
        };
        repo.set(&s).unwrap();
        assert_eq!(repo.get().unwrap().unwrap(), s);

        // Second write replaces, doesn't append.
        let s2 = TuiState {
            tab: "Questions".into(),
            paper_id: None,
            search_id: None,
            question_id: Some("q-xyz".into()),
            annotation_id: None,
            updated_at: "2026-04-20T00:01:00Z".into(),
        };
        repo.set(&s2).unwrap();
        assert_eq!(repo.get().unwrap().unwrap(), s2);
    }
}
