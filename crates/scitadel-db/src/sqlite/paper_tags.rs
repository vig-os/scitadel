//! Paper-level tags (#162 — keyword-only Zotero imports).
//!
//! Stores free-form tags attached to a paper. Mirrors the shape of
//! `paper_aliases` (#134 / migration 011): `(paper_id, value)` PK,
//! `source` audit column, idempotent `record`, ordered `list_for`.
//! See migration 012 for schema rationale — keyword-only Zotero
//! entries used to be dropped on the floor; this is where they land.

use rusqlite::params;

use crate::error::DbError;
use crate::sqlite::Database;

/// Free-form source tag recorded alongside a paper-tag row. Matches
/// the alias counterpart so audit queries can grep by source across
/// both tables. Re-declared here for symmetry with `paper_aliases`.
pub const TAG_SOURCE_BIBTEX_IMPORT: &str = "bibtex-import";

#[derive(Clone)]
pub struct SqlitePaperTagRepository {
    db: Database,
}

impl SqlitePaperTagRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Record a tag. Idempotent on `(paper_id, tag)` — re-running an
    /// import that already added the tag is a no-op. Returns `true`
    /// if a new row was inserted, `false` if it already existed.
    pub fn record(&self, paper_id: &str, tag: &str, source: &str) -> Result<bool, DbError> {
        let conn = self.db.conn()?;
        let rows = conn.execute(
            "INSERT OR IGNORE INTO paper_tags (paper_id, tag, source, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![paper_id, tag, source, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(rows > 0)
    }

    /// Transactional sibling of [`Self::record`] (#157). Used by the
    /// bib-import orchestrator so paper-save + alias / annotation /
    /// tag writes commit (or roll back) as a single unit per row.
    pub fn record_in_tx(
        tx: &rusqlite::Transaction<'_>,
        paper_id: &str,
        tag: &str,
        source: &str,
    ) -> Result<bool, DbError> {
        let rows = tx.execute(
            "INSERT OR IGNORE INTO paper_tags (paper_id, tag, source, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![paper_id, tag, source, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(rows > 0)
    }

    /// All tags for a paper, in insertion order. Returned as
    /// `(tag, source)` so callers can distinguish bibtex-import-
    /// originated tags from manually-added ones.
    pub fn list_for(&self, paper_id: &str) -> Result<Vec<(String, String)>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT tag, source FROM paper_tags
             WHERE paper_id = ?1
             ORDER BY created_at ASC, tag ASC",
        )?;
        let rows = stmt.query_map(params![paper_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Tag names only, in insertion order. Convenience for the export
    /// path which only needs the strings (no source metadata).
    pub fn tags_for(&self, paper_id: &str) -> Result<Vec<String>, DbError> {
        Ok(self
            .list_for(paper_id)?
            .into_iter()
            .map(|(t, _)| t)
            .collect())
    }

    /// All paper IDs carrying a tag, in deterministic order. Mirrors
    /// `SqlitePaperAliasRepository::lookup_all` so a future "filter by
    /// tag" UI can pick this up without a new method.
    pub fn lookup_all(&self, tag: &str) -> Result<Vec<String>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT paper_id FROM paper_tags
             WHERE tag = ?1
             ORDER BY created_at ASC, paper_id ASC",
        )?;
        let rows = stmt.query_map(params![tag], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> SqlitePaperTagRepository {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let conn = db.conn().unwrap();
        for id in ["p1", "p2"] {
            conn.execute(
                "INSERT INTO papers (id, title, created_at, updated_at)
                 VALUES (?1, 'T', datetime('now'), datetime('now'))",
                params![id],
            )
            .unwrap();
        }
        drop(conn);
        SqlitePaperTagRepository::new(db)
    }

    #[test]
    fn record_and_list_round_trip() {
        let repo = fresh();
        assert!(
            repo.record("p1", "machine-learning", TAG_SOURCE_BIBTEX_IMPORT)
                .unwrap()
        );
        let tags = repo.list_for("p1").unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].0, "machine-learning");
        assert_eq!(tags[0].1, TAG_SOURCE_BIBTEX_IMPORT);
    }

    #[test]
    fn record_is_idempotent_on_paper_id_tag() {
        let repo = fresh();
        assert!(
            repo.record("p1", "alpha", TAG_SOURCE_BIBTEX_IMPORT)
                .unwrap()
        );
        assert!(
            !repo
                .record("p1", "alpha", TAG_SOURCE_BIBTEX_IMPORT)
                .unwrap(),
            "second insert returns false"
        );
        assert_eq!(repo.list_for("p1").unwrap().len(), 1);
    }

    #[test]
    fn same_tag_can_attach_to_two_different_papers() {
        let repo = fresh();
        assert!(
            repo.record("p1", "shared", TAG_SOURCE_BIBTEX_IMPORT)
                .unwrap()
        );
        assert!(
            repo.record("p2", "shared", TAG_SOURCE_BIBTEX_IMPORT)
                .unwrap()
        );
        let papers = repo.lookup_all("shared").unwrap();
        assert_eq!(papers.len(), 2);
        assert!(papers.contains(&"p1".to_string()));
        assert!(papers.contains(&"p2".to_string()));
    }

    #[test]
    fn cascade_delete_removes_orphan_tags() {
        let repo = fresh();
        repo.record("p1", "k1", TAG_SOURCE_BIBTEX_IMPORT).unwrap();
        repo.record("p1", "k2", TAG_SOURCE_BIBTEX_IMPORT).unwrap();
        {
            let conn = repo.db.conn().unwrap();
            conn.execute("DELETE FROM papers WHERE id = 'p1'", [])
                .unwrap();
        }
        assert!(repo.list_for("p1").unwrap().is_empty());
        assert!(repo.lookup_all("k1").unwrap().is_empty());
    }

    #[test]
    fn list_for_preserves_insertion_order() {
        let repo = fresh();
        repo.record("p1", "first", TAG_SOURCE_BIBTEX_IMPORT)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        repo.record("p1", "second", "manual").unwrap();
        let tags = repo.list_for("p1").unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].0, "first");
        assert_eq!(tags[0].1, TAG_SOURCE_BIBTEX_IMPORT);
        assert_eq!(tags[1].0, "second");
        assert_eq!(tags[1].1, "manual");
    }

    #[test]
    fn tags_for_returns_strings_only() {
        let repo = fresh();
        repo.record("p1", "alpha", TAG_SOURCE_BIBTEX_IMPORT)
            .unwrap();
        repo.record("p1", "beta", TAG_SOURCE_BIBTEX_IMPORT).unwrap();
        let tags = repo.tags_for("p1").unwrap();
        assert_eq!(tags, vec!["alpha".to_string(), "beta".to_string()]);
    }
}
