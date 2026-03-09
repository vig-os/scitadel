use rusqlite::{params, OptionalExtension};
use scitadel_core::error::CoreError;
use scitadel_core::models::{
    PaperId, Search, SearchId, SearchResult, SourceOutcome,
};
use scitadel_core::ports::SearchRepository;

use super::Database;
use crate::error::DbError;

pub struct SqliteSearchRepository {
    db: Database,
}

impl SqliteSearchRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn row_to_search(row: &rusqlite::Row) -> rusqlite::Result<Search> {
    let id: String = row.get("id")?;
    let sources_json: String = row.get("sources")?;
    let parameters_json: String = row.get("parameters")?;
    let outcomes_json: String = row.get("source_outcomes")?;
    let created_at: String = row.get("created_at")?;

    let outcomes: Vec<SourceOutcome> = serde_json::from_str(&outcomes_json).unwrap_or_default();

    Ok(Search {
        id: SearchId::from(id),
        query: row.get("query")?,
        sources: serde_json::from_str(&sources_json).unwrap_or_default(),
        parameters: serde_json::from_str(&parameters_json).unwrap_or_default(),
        source_outcomes: outcomes,
        total_candidates: row.get("total_candidates")?,
        total_papers: row.get("total_papers")?,
        created_at: super::parse_rfc3339_or_now(&created_at),
    })
}

impl SearchRepository for SqliteSearchRepository {
    fn save(&self, search: &Search) -> Result<(), CoreError> {
        let conn = self.db.conn()?;
        let outcomes_json = serde_json::to_string(&search.source_outcomes).unwrap_or_default();
        conn.execute(
            "INSERT INTO searches
                (id, query, sources, parameters, source_outcomes,
                 total_candidates, total_papers, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                query = excluded.query,
                sources = excluded.sources,
                parameters = excluded.parameters,
                source_outcomes = excluded.source_outcomes,
                total_candidates = excluded.total_candidates,
                total_papers = excluded.total_papers",
            params![
                search.id.as_str(),
                search.query,
                serde_json::to_string(&search.sources).unwrap_or_default(),
                serde_json::to_string(&search.parameters).unwrap_or_default(),
                outcomes_json,
                search.total_candidates,
                search.total_papers,
                search.created_at.to_rfc3339(),
            ],
        )
        .map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn get(&self, search_id: &str) -> Result<Option<Search>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM searches WHERE id = ?1")
            .map_err(DbError::Sqlite)?;
        let result = stmt
            .query_row(params![search_id], row_to_search)
            .optional()
            .map_err(DbError::Sqlite)?;
        Ok(result)
    }

    fn save_results(&self, results: &[SearchResult]) -> Result<(), CoreError> {
        let mut conn = self.db.conn()?;
        let tx = conn.transaction().map_err(DbError::Sqlite)?;
        for r in results {
            tx.execute(
                "INSERT INTO search_results
                    (search_id, paper_id, source, rank, score, raw_metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(search_id, paper_id, source) DO UPDATE SET
                    rank = excluded.rank,
                    score = excluded.score,
                    raw_metadata = excluded.raw_metadata",
                params![
                    r.search_id.as_str(),
                    r.paper_id.as_str(),
                    r.source,
                    r.rank,
                    r.score,
                    serde_json::to_string(&r.raw_metadata).unwrap_or_default(),
                ],
            )
            .map_err(DbError::Sqlite)?;
        }
        tx.commit().map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn get_results(&self, search_id: &str) -> Result<Vec<SearchResult>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM search_results WHERE search_id = ?1")
            .map_err(DbError::Sqlite)?;
        let results = stmt
            .query_map(params![search_id], |row| {
                let search_id: String = row.get("search_id")?;
                let paper_id: String = row.get("paper_id")?;
                let raw_json: String = row.get("raw_metadata")?;
                Ok(SearchResult {
                    search_id: SearchId::from(search_id),
                    paper_id: PaperId::from(paper_id),
                    source: row.get("source")?,
                    rank: row.get("rank")?,
                    score: row.get("score")?,
                    raw_metadata: serde_json::from_str(&raw_json).unwrap_or_default(),
                })
            })
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(results)
    }

    fn list_searches(&self, limit: i64) -> Result<Vec<Search>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM searches ORDER BY created_at DESC LIMIT ?1")
            .map_err(DbError::Sqlite)?;
        let searches = stmt
            .query_map(params![limit], row_to_search)
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(searches)
    }

    fn diff_searches(
        &self,
        search_id_a: &str,
        search_id_b: &str,
    ) -> Result<(Vec<String>, Vec<String>), CoreError> {
        let conn = self.db.conn()?;

        let get_paper_ids = |search_id: &str| -> Result<std::collections::HashSet<String>, DbError> {
            let mut stmt = conn
                .prepare("SELECT DISTINCT paper_id FROM search_results WHERE search_id = ?1")
                .map_err(DbError::Sqlite)?;
            let ids: std::collections::HashSet<String> = stmt
                .query_map(params![search_id], |row| row.get(0))
                .map_err(DbError::Sqlite)?
                .filter_map(Result::ok)
                .collect();
            Ok(ids)
        };

        let papers_a = get_paper_ids(search_id_a).map_err(Into::<CoreError>::into)?;
        let papers_b = get_paper_ids(search_id_b).map_err(Into::<CoreError>::into)?;

        let mut added: Vec<String> = papers_b.difference(&papers_a).cloned().collect();
        let mut removed: Vec<String> = papers_a.difference(&papers_b).cloned().collect();
        added.sort();
        removed.sort();

        Ok((added, removed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::{Database, SqlitePaperRepository};
    use scitadel_core::models::Paper;
    use scitadel_core::ports::PaperRepository;

    fn setup() -> (Database, SqliteSearchRepository, SqlitePaperRepository) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let search_repo = SqliteSearchRepository::new(db.clone());
        let paper_repo = SqlitePaperRepository::new(db.clone());
        (db, search_repo, paper_repo)
    }

    #[test]
    fn test_save_and_get_search() {
        let (_, repo, _) = setup();
        let search = Search::new("test query");
        repo.save(&search).unwrap();

        let loaded = repo.get(search.id.as_str()).unwrap().unwrap();
        assert_eq!(loaded.query, "test query");
    }

    #[test]
    fn test_save_and_get_results() {
        let (_, search_repo, paper_repo) = setup();

        let paper = Paper::new("Test Paper");
        paper_repo.save(&paper).unwrap();

        let search = Search::new("test");
        search_repo.save(&search).unwrap();

        let result = SearchResult {
            search_id: search.id.clone(),
            paper_id: paper.id.clone(),
            source: "pubmed".to_string(),
            rank: Some(1),
            score: Some(0.95),
            raw_metadata: serde_json::Value::Null,
        };
        search_repo.save_results(&[result]).unwrap();

        let results = search_repo.get_results(search.id.as_str()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "pubmed");
    }
}
