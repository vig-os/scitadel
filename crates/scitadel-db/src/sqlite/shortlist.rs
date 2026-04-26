//! Per-(question, reader) citation shortlist (#133).
//!
//! Curated set of papers the reader will cite for a research question.
//! Drives the Question Dashboard's `c` keybind and feeds
//! `bib snapshot <question_id>` (#134, 0.6.1).
//!
//! Multi-author story: `reader` scope mirrors the annotation and star
//! conventions — per-reader shortlists diverge until Dolt sync lands
//! in Phase 5.

use rusqlite::{OptionalExtension, params};

use crate::error::DbError;
use crate::sqlite::Database;

#[derive(Clone)]
pub struct SqliteShortlistRepository {
    db: Database,
}

impl SqliteShortlistRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Is this paper on `reader`'s shortlist for `question_id`?
    pub fn contains(
        &self,
        question_id: &str,
        paper_id: &str,
        reader: &str,
    ) -> Result<bool, DbError> {
        let conn = self.db.conn()?;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM shortlist_members
                 WHERE question_id = ?1 AND paper_id = ?2 AND reader = ?3",
                params![question_id, paper_id, reader],
                |r| r.get(0),
            )
            .optional()?;
        Ok(exists.is_some())
    }

    /// Toggle membership. Returns the post-toggle state (`true` = on
    /// shortlist). Matches the TUI's `c` keybind and MCP
    /// `toggle_shortlist` semantics. Does the whole check-and-swap
    /// under one pool connection to avoid deadlocking the single-
    /// connection in-memory test DB.
    pub fn toggle(&self, question_id: &str, paper_id: &str, reader: &str) -> Result<bool, DbError> {
        let conn = self.db.conn()?;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM shortlist_members
                 WHERE question_id = ?1 AND paper_id = ?2 AND reader = ?3",
                params![question_id, paper_id, reader],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_some() {
            conn.execute(
                "DELETE FROM shortlist_members
                 WHERE question_id = ?1 AND paper_id = ?2 AND reader = ?3",
                params![question_id, paper_id, reader],
            )?;
            Ok(false)
        } else {
            conn.execute(
                "INSERT INTO shortlist_members (question_id, paper_id, reader, added_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    question_id,
                    paper_id,
                    reader,
                    chrono::Utc::now().to_rfc3339()
                ],
            )?;
            Ok(true)
        }
    }

    /// Paper IDs on `reader`'s shortlist for `question_id`, in
    /// added-at-ascending order (the order the reader built the list).
    pub fn list(&self, question_id: &str, reader: &str) -> Result<Vec<String>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT paper_id FROM shortlist_members
             WHERE question_id = ?1 AND reader = ?2
             ORDER BY added_at ASC",
        )?;
        let rows = stmt.query_map(params![question_id, reader], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// `paper_id` set for use as a contains-check in render loops —
    /// cheaper than N per-row `contains` calls.
    pub fn members_set(
        &self,
        question_id: &str,
        reader: &str,
    ) -> Result<std::collections::HashSet<String>, DbError> {
        Ok(self.list(question_id, reader)?.into_iter().collect())
    }

    /// Latest update timestamp across the tables that the `bib watch`
    /// engine considers shared-document state for `question_id`:
    /// `papers` rows referenced by the shortlist, `paper_state` rows
    /// for those papers, and `shortlist_members` membership changes.
    /// Returns `None` when the question has no shortlist or all
    /// touched tables are empty. Caller compares string equality
    /// (RFC3339 timestamps are lex-comparable per ISO 8601).
    ///
    /// Note: stars are personal-view state and are filtered at the
    /// watch-engine layer, NOT excluded here — the engine ignores
    /// changes whose only signal is `paper_state.starred` flipping.
    /// This query also doesn't join `annotations` directly: the
    /// import-side bib-content build folds annotation `note=` text
    /// through the paper row, so a meaningful annotation change
    /// surfaces via `papers.updated_at`.
    pub fn max_updated_at_for_question(
        &self,
        question_id: &str,
    ) -> Result<Option<String>, DbError> {
        let conn = self.db.conn()?;
        let ts: Option<String> = conn
            .query_row(
                "SELECT MAX(t) FROM (
                    SELECT MAX(updated_at) AS t FROM papers
                        WHERE id IN (SELECT paper_id FROM shortlist_members
                                     WHERE question_id = ?1)
                    UNION ALL
                    SELECT MAX(updated_at) AS t FROM paper_state
                        WHERE paper_id IN (SELECT paper_id FROM shortlist_members
                                           WHERE question_id = ?1)
                    UNION ALL
                    SELECT MAX(added_at) AS t FROM shortlist_members
                        WHERE question_id = ?1
                 )",
                params![question_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(ts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> SqliteShortlistRepository {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let conn = db.conn().unwrap();
        // Need a question + a paper to satisfy FKs.
        conn.execute(
            "INSERT INTO research_questions (id, text, description, created_at, updated_at)
             VALUES ('q1', 'Q', '', datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO papers (id, title, created_at, updated_at)
             VALUES ('p1', 'T', datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();
        SqliteShortlistRepository::new(db)
    }

    #[test]
    fn toggle_round_trip() {
        let repo = fresh();
        assert!(repo.toggle("q1", "p1", "lars").unwrap(), "adds on first");
        assert!(repo.contains("q1", "p1", "lars").unwrap());
        assert!(
            !repo.toggle("q1", "p1", "lars").unwrap(),
            "removes on second"
        );
        assert!(!repo.contains("q1", "p1", "lars").unwrap());
    }

    #[test]
    fn list_is_added_at_ascending() {
        let repo = fresh();
        // Insert the second paper in a scoped block so the connection
        // is released before `repo.toggle` needs one (the test DB's
        // pool is single-connection).
        {
            let conn = repo.db.conn().unwrap();
            conn.execute(
                "INSERT INTO papers (id, title, created_at, updated_at)
                 VALUES ('p2', 'T2', datetime('now'), datetime('now'))",
                [],
            )
            .unwrap();
        }
        repo.toggle("q1", "p2", "lars").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        repo.toggle("q1", "p1", "lars").unwrap();
        let ids = repo.list("q1", "lars").unwrap();
        assert_eq!(ids, vec!["p2", "p1"], "oldest first");
    }

    #[test]
    fn readers_have_independent_shortlists() {
        let repo = fresh();
        repo.toggle("q1", "p1", "lars").unwrap();
        assert!(!repo.contains("q1", "p1", "claude").unwrap());
        assert!(repo.contains("q1", "p1", "lars").unwrap());
    }
}
