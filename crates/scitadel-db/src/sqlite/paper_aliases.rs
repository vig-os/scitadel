//! Paper citation aliases (#134 — bib import iter 2).
//!
//! Stores alternative citekeys a paper is known by (typically the
//! citekey from an imported `.bib` file). See migration 011 for the
//! schema rationale. Authoritative key stays on `papers.bibtex_key`;
//! aliases are additive and never rename the paper.

use rusqlite::{OptionalExtension, params};

use crate::error::DbError;
use crate::sqlite::Database;

/// Free-form source tag recorded alongside an alias.
pub const SOURCE_BIBTEX_IMPORT: &str = "bibtex-import";

#[derive(Clone)]
pub struct SqlitePaperAliasRepository {
    db: Database,
}

impl SqlitePaperAliasRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Record an alias. Idempotent on `(paper_id, alias)` — re-running
    /// an import that already added the alias is a no-op. Returns
    /// `true` if a new row was inserted, `false` if it already existed.
    pub fn record(&self, paper_id: &str, alias: &str, source: &str) -> Result<bool, DbError> {
        let conn = self.db.conn()?;
        let rows = conn.execute(
            "INSERT OR IGNORE INTO paper_aliases (paper_id, alias, source, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![paper_id, alias, source, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(rows > 0)
    }

    /// Look up a paper by alias. When two papers share an alias (legal —
    /// two different imports may collide on citekey) this returns the
    /// earliest-created row so the lookup is deterministic. Secondary
    /// `paper_id ASC` tiebreak covers same-microsecond inserts during
    /// batch imports where `created_at` ties are realistic.
    ///
    /// For the match pipeline, prefer [`Self::lookup_all`] so you can
    /// explicitly detect ambiguity and fall through to other match
    /// strategies rather than silently picking the first row.
    pub fn lookup(&self, alias: &str) -> Result<Option<String>, DbError> {
        let conn = self.db.conn()?;
        let paper_id: Option<String> = conn
            .query_row(
                "SELECT paper_id FROM paper_aliases
                 WHERE alias = ?1
                 ORDER BY created_at ASC, paper_id ASC
                 LIMIT 1",
                params![alias],
                |r| r.get(0),
            )
            .optional()?;
        Ok(paper_id)
    }

    /// All paper IDs sharing an alias, in deterministic order. The
    /// match pipeline uses this to detect ambiguity (`len() > 1` →
    /// skip the alias strategy, fall through to DOI/arxiv/etc).
    pub fn lookup_all(&self, alias: &str) -> Result<Vec<String>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT paper_id FROM paper_aliases
             WHERE alias = ?1
             ORDER BY created_at ASC, paper_id ASC",
        )?;
        let rows = stmt.query_map(params![alias], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// All aliases for a paper, in insertion order.
    pub fn list_for(&self, paper_id: &str) -> Result<Vec<(String, String)>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT alias, source FROM paper_aliases
             WHERE paper_id = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![paper_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> SqlitePaperAliasRepository {
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
        SqlitePaperAliasRepository::new(db)
    }

    #[test]
    fn record_and_lookup_round_trip() {
        let repo = fresh();
        assert!(
            repo.record("p1", "smith2024old", SOURCE_BIBTEX_IMPORT)
                .unwrap()
        );
        assert_eq!(repo.lookup("smith2024old").unwrap().as_deref(), Some("p1"));
        assert_eq!(repo.lookup("nonexistent").unwrap(), None);
    }

    #[test]
    fn record_is_idempotent_on_paper_id_alias() {
        let repo = fresh();
        assert!(repo.record("p1", "k", SOURCE_BIBTEX_IMPORT).unwrap());
        assert!(
            !repo.record("p1", "k", SOURCE_BIBTEX_IMPORT).unwrap(),
            "second insert returns false"
        );
        assert_eq!(repo.list_for("p1").unwrap().len(), 1);
    }

    #[test]
    fn alias_collision_earliest_created_wins() {
        let repo = fresh();
        repo.record("p2", "shared2024", SOURCE_BIBTEX_IMPORT)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        repo.record("p1", "shared2024", SOURCE_BIBTEX_IMPORT)
            .unwrap();
        assert_eq!(
            repo.lookup("shared2024").unwrap().as_deref(),
            Some("p2"),
            "earliest-created row wins"
        );
        let all = repo.lookup_all("shared2024").unwrap();
        assert_eq!(all, vec!["p2".to_string(), "p1".to_string()]);
    }

    /// Paper-id ASC is the secondary tiebreak when `created_at` ties.
    /// Batch imports making hundreds of `record()` calls can easily
    /// collide on timestamps (even with nanosecond precision under
    /// sccache-parallel test runs). Force the tie via direct SQL.
    #[test]
    fn alias_collision_same_timestamp_tiebreaks_on_paper_id_asc() {
        let repo = fresh();
        let same_ts = "2026-04-24T12:00:00Z";
        let conn = repo.db.conn().unwrap();
        conn.execute(
            "INSERT INTO paper_aliases (paper_id, alias, source, created_at)
             VALUES ('p2', 'shared2024', 'bibtex-import', ?1),
                    ('p1', 'shared2024', 'bibtex-import', ?1)",
            params![same_ts],
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            repo.lookup("shared2024").unwrap().as_deref(),
            Some("p1"),
            "paper_id ASC breaks the tie when timestamps are identical"
        );
        let all = repo.lookup_all("shared2024").unwrap();
        assert_eq!(all, vec!["p1".to_string(), "p2".to_string()]);
    }

    #[test]
    fn cascade_delete_removes_orphan_aliases() {
        let repo = fresh();
        repo.record("p1", "k1", SOURCE_BIBTEX_IMPORT).unwrap();
        repo.record("p1", "k2", SOURCE_BIBTEX_IMPORT).unwrap();
        {
            let conn = repo.db.conn().unwrap();
            conn.execute("DELETE FROM papers WHERE id = 'p1'", [])
                .unwrap();
        }
        assert_eq!(repo.lookup("k1").unwrap(), None);
        assert_eq!(repo.lookup("k2").unwrap(), None);
    }

    #[test]
    fn list_for_returns_all_aliases_in_order() {
        let repo = fresh();
        repo.record("p1", "first", SOURCE_BIBTEX_IMPORT).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        repo.record("p1", "second", "manual").unwrap();
        let aliases = repo.list_for("p1").unwrap();
        assert_eq!(aliases.len(), 2);
        assert_eq!(aliases[0].0, "first");
        assert_eq!(aliases[0].1, SOURCE_BIBTEX_IMPORT);
        assert_eq!(aliases[1].0, "second");
        assert_eq!(aliases[1].1, "manual");
    }
}
