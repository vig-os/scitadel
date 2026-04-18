use rusqlite::{OptionalExtension, params};
use scitadel_core::error::CoreError;
use scitadel_core::models::{
    Citation, CitationDirection, PaperId, QuestionId, SearchId, SnowballRun, SnowballRunId,
};
use scitadel_core::ports::CitationRepository;

use super::Database;
use crate::error::DbError;

const UPSERT_SQL: &str = "\
    INSERT INTO citations
        (source_paper_id, target_paper_id, direction,
         discovered_by, depth, snowball_run_id)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6)
    ON CONFLICT(source_paper_id, target_paper_id, direction) DO UPDATE SET
        depth = MIN(citations.depth, excluded.depth),
        snowball_run_id = COALESCE(excluded.snowball_run_id,
                                   citations.snowball_run_id)";

pub struct SqliteCitationRepository {
    db: Database,
}

impl SqliteCitationRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn row_to_citation(row: &rusqlite::Row) -> rusqlite::Result<Citation> {
    let source_paper_id: String = row.get("source_paper_id")?;
    let target_paper_id: String = row.get("target_paper_id")?;
    let direction: String = row.get("direction")?;
    let snowball_run_id: Option<String> = row.get("snowball_run_id")?;

    Ok(Citation {
        source_paper_id: PaperId::from(source_paper_id),
        target_paper_id: PaperId::from(target_paper_id),
        direction: CitationDirection::from_str_value(&direction)
            .unwrap_or(CitationDirection::References),
        discovered_by: row.get("discovered_by")?,
        depth: row.get("depth")?,
        snowball_run_id: snowball_run_id.map(SnowballRunId::from),
    })
}

fn row_to_snowball_run(row: &rusqlite::Row) -> rusqlite::Result<SnowballRun> {
    let id: String = row.get("id")?;
    let search_id: Option<String> = row.get("search_id")?;
    let question_id: Option<String> = row.get("question_id")?;
    let created_at: String = row.get("created_at")?;

    Ok(SnowballRun {
        id: SnowballRunId::from(id),
        search_id: search_id.map(SearchId::from),
        question_id: question_id.map(QuestionId::from),
        direction: row.get("direction")?,
        max_depth: row.get("max_depth")?,
        threshold: row.get("threshold")?,
        total_discovered: row.get("total_discovered")?,
        total_new_papers: row.get("total_new_papers")?,
        created_at: super::parse_rfc3339_or_now(&created_at),
    })
}

impl CitationRepository for SqliteCitationRepository {
    fn save(&self, citation: &Citation) -> Result<(), CoreError> {
        let conn = self.db.conn()?;
        conn.execute(
            UPSERT_SQL,
            params![
                citation.source_paper_id.as_str(),
                citation.target_paper_id.as_str(),
                citation.direction.to_string(),
                citation.discovered_by,
                citation.depth,
                citation
                    .snowball_run_id
                    .as_ref()
                    .map(|id| id.as_str().to_string()),
            ],
        )
        .map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn save_many(&self, citations: &[Citation]) -> Result<(), CoreError> {
        let conn = self.db.conn()?;
        for c in citations {
            conn.execute(
                UPSERT_SQL,
                params![
                    c.source_paper_id.as_str(),
                    c.target_paper_id.as_str(),
                    c.direction.to_string(),
                    c.discovered_by,
                    c.depth,
                    c.snowball_run_id.as_ref().map(|id| id.as_str().to_string()),
                ],
            )
            .map_err(DbError::Sqlite)?;
        }
        Ok(())
    }

    fn get_references(&self, paper_id: &str) -> Result<Vec<Citation>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM citations WHERE source_paper_id = ?1 AND direction = ?2")
            .map_err(DbError::Sqlite)?;
        let citations = stmt
            .query_map(
                params![paper_id, CitationDirection::References.to_string()],
                row_to_citation,
            )
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(citations)
    }

    fn get_citations(&self, paper_id: &str) -> Result<Vec<Citation>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM citations WHERE target_paper_id = ?1 AND direction = ?2")
            .map_err(DbError::Sqlite)?;
        let citations = stmt
            .query_map(
                params![paper_id, CitationDirection::CitedBy.to_string()],
                row_to_citation,
            )
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(citations)
    }

    fn exists(
        &self,
        source_paper_id: &str,
        target_paper_id: &str,
        direction: &str,
    ) -> Result<bool, CoreError> {
        let conn = self.db.conn()?;
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM citations WHERE source_paper_id = ?1 AND target_paper_id = ?2 AND direction = ?3)",
                params![source_paper_id, target_paper_id, direction],
                |row| row.get(0),
            )
            .map_err(DbError::Sqlite)?;
        Ok(exists)
    }

    fn save_snowball_run(&self, run: &SnowballRun) -> Result<(), CoreError> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO snowball_runs
                (id, search_id, question_id, direction, max_depth,
                 threshold, total_discovered, total_new_papers, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run.id.as_str(),
                run.search_id.as_ref().map(|id| id.as_str().to_string()),
                run.question_id.as_ref().map(|id| id.as_str().to_string()),
                run.direction,
                run.max_depth,
                run.threshold,
                run.total_discovered,
                run.total_new_papers,
                run.created_at.to_rfc3339(),
            ],
        )
        .map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn get_snowball_run(&self, run_id: &str) -> Result<Option<SnowballRun>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM snowball_runs WHERE id = ?1")
            .map_err(DbError::Sqlite)?;
        let result = stmt
            .query_row(params![run_id], row_to_snowball_run)
            .optional()
            .map_err(DbError::Sqlite)?;
        Ok(result)
    }

    fn list_snowball_runs(&self, limit: i64) -> Result<Vec<SnowballRun>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM snowball_runs ORDER BY created_at DESC LIMIT ?1")
            .map_err(DbError::Sqlite)?;
        let runs = stmt
            .query_map(params![limit], row_to_snowball_run)
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::{Database, SqlitePaperRepository};
    use scitadel_core::models::Paper;
    use scitadel_core::ports::PaperRepository;

    #[test]
    fn test_citation_crud() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let paper_repo = SqlitePaperRepository::new(db.clone());
        let citation_repo = SqliteCitationRepository::new(db);

        let paper_a = Paper::new("Paper A");
        let paper_b = Paper::new("Paper B");
        paper_repo.save(&paper_a).unwrap();
        paper_repo.save(&paper_b).unwrap();

        let citation = Citation {
            source_paper_id: paper_a.id.clone(),
            target_paper_id: paper_b.id.clone(),
            direction: CitationDirection::References,
            discovered_by: "openalex".to_string(),
            depth: 1,
            snowball_run_id: None,
        };
        citation_repo.save(&citation).unwrap();

        let refs = citation_repo.get_references(paper_a.id.as_str()).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].target_paper_id, paper_b.id);

        assert!(
            citation_repo
                .exists(paper_a.id.as_str(), paper_b.id.as_str(), "references")
                .unwrap()
        );
    }

    #[test]
    fn test_snowball_run_crud() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let repo = SqliteCitationRepository::new(db);

        let run = SnowballRun::new();
        repo.save_snowball_run(&run).unwrap();

        let loaded = repo.get_snowball_run(run.id.as_str()).unwrap().unwrap();
        assert_eq!(loaded.direction, "both");
    }
}
