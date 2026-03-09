use rusqlite::params;
use scitadel_core::error::CoreError;
use scitadel_core::models::{Paper, PaperId};
use scitadel_core::ports::PaperRepository;

use super::Database;
use crate::error::DbError;

const UPSERT_SQL: &str = "\
    INSERT INTO papers
        (id, title, authors, abstract, full_text, summary, doi, arxiv_id,
         pubmed_id, inspire_id, openalex_id, year, journal, url,
         source_urls, created_at, updated_at)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
    ON CONFLICT(id) DO UPDATE SET
        title      = excluded.title,
        authors    = excluded.authors,
        abstract   = CASE WHEN excluded.abstract != '' THEN excluded.abstract
                          ELSE papers.abstract END,
        full_text  = COALESCE(excluded.full_text, papers.full_text),
        summary    = COALESCE(excluded.summary, papers.summary),
        doi        = COALESCE(excluded.doi, papers.doi),
        arxiv_id   = COALESCE(excluded.arxiv_id, papers.arxiv_id),
        pubmed_id  = COALESCE(excluded.pubmed_id, papers.pubmed_id),
        inspire_id = COALESCE(excluded.inspire_id, papers.inspire_id),
        openalex_id= COALESCE(excluded.openalex_id, papers.openalex_id),
        year       = COALESCE(excluded.year, papers.year),
        journal    = COALESCE(excluded.journal, papers.journal),
        url        = COALESCE(excluded.url, papers.url),
        source_urls= excluded.source_urls,
        updated_at = excluded.updated_at";

pub struct SqlitePaperRepository {
    db: Database,
}

impl SqlitePaperRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    fn paper_params(paper: &Paper) -> [Box<dyn rusqlite::types::ToSql>; 17] {
        [
            Box::new(paper.id.as_str().to_string()),
            Box::new(paper.title.clone()),
            Box::new(serde_json::to_string(&paper.authors).unwrap_or_default()),
            Box::new(paper.r#abstract.clone()),
            Box::new(paper.full_text.clone()),
            Box::new(paper.summary.clone()),
            Box::new(paper.doi.clone()),
            Box::new(paper.arxiv_id.clone()),
            Box::new(paper.pubmed_id.clone()),
            Box::new(paper.inspire_id.clone()),
            Box::new(paper.openalex_id.clone()),
            Box::new(paper.year),
            Box::new(paper.journal.clone()),
            Box::new(paper.url.clone()),
            Box::new(serde_json::to_string(&paper.source_urls).unwrap_or_default()),
            Box::new(paper.created_at.to_rfc3339()),
            Box::new(paper.updated_at.to_rfc3339()),
        ]
    }
}

fn row_to_paper(row: &rusqlite::Row) -> rusqlite::Result<Paper> {
    let id: String = row.get("id")?;
    let authors_json: String = row.get("authors")?;
    let source_urls_json: String = row.get("source_urls")?;
    let created_at: String = row.get("created_at")?;
    let updated_at: String = row.get("updated_at")?;

    Ok(Paper {
        id: PaperId::from(id),
        title: row.get("title")?,
        authors: serde_json::from_str(&authors_json).unwrap_or_default(),
        r#abstract: row.get("abstract")?,
        full_text: row.get("full_text")?,
        summary: row.get("summary")?,
        doi: row.get("doi")?,
        arxiv_id: row.get("arxiv_id")?,
        pubmed_id: row.get("pubmed_id")?,
        inspire_id: row.get("inspire_id")?,
        openalex_id: row.get("openalex_id")?,
        year: row.get("year")?,
        journal: row.get("journal")?,
        url: row.get("url")?,
        source_urls: serde_json::from_str(&source_urls_json).unwrap_or_default(),
        created_at: super::parse_rfc3339_or_now(&created_at),
        updated_at: super::parse_rfc3339_or_now(&updated_at),
    })
}

impl PaperRepository for SqlitePaperRepository {
    fn save(&self, paper: &Paper) -> Result<(), CoreError> {
        let conn = self.db.conn()?;
        let p = Self::paper_params(paper);
        let params: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        conn.execute(UPSERT_SQL, params.as_slice())
            .map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn save_many(&self, papers: &[Paper]) -> Result<(), CoreError> {
        let mut conn = self.db.conn()?;
        let tx = conn.transaction().map_err(DbError::Sqlite)?;
        for paper in papers {
            let p = Self::paper_params(paper);
            let params: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|b| b.as_ref()).collect();
            tx.execute(UPSERT_SQL, params.as_slice())
                .map_err(DbError::Sqlite)?;
        }
        tx.commit().map_err(DbError::Sqlite)?;
        Ok(())
    }

    fn get(&self, paper_id: &str) -> Result<Option<Paper>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM papers WHERE id = ?1")
            .map_err(DbError::Sqlite)?;
        let result = stmt
            .query_row(params![paper_id], row_to_paper)
            .optional()
            .map_err(DbError::Sqlite)?;
        Ok(result)
    }

    fn find_by_doi(&self, doi: &str) -> Result<Option<Paper>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM papers WHERE doi = ?1")
            .map_err(DbError::Sqlite)?;
        let result = stmt
            .query_row(params![doi], row_to_paper)
            .optional()
            .map_err(DbError::Sqlite)?;
        Ok(result)
    }

    fn find_by_title(&self, title: &str) -> Result<Option<Paper>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM papers WHERE LOWER(title) = LOWER(?1)")
            .map_err(DbError::Sqlite)?;
        let result = stmt
            .query_row(params![title], row_to_paper)
            .optional()
            .map_err(DbError::Sqlite)?;
        Ok(result)
    }

    fn list_all(&self, limit: i64, offset: i64) -> Result<Vec<Paper>, CoreError> {
        let conn = self.db.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM papers ORDER BY created_at DESC LIMIT ?1 OFFSET ?2")
            .map_err(DbError::Sqlite)?;
        let papers = stmt
            .query_map(params![limit, offset], row_to_paper)
            .map_err(DbError::Sqlite)?
            .filter_map(Result::ok)
            .collect();
        Ok(papers)
    }
}

// Need this import for .optional()
use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::Database;

    fn setup() -> (Database, SqlitePaperRepository) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let repo = SqlitePaperRepository::new(db.clone());
        (db, repo)
    }

    #[test]
    fn test_save_and_get() {
        let (_, repo) = setup();
        let paper = Paper::new("Test Paper");
        repo.save(&paper).unwrap();

        let loaded = repo.get(paper.id.as_str()).unwrap().unwrap();
        assert_eq!(loaded.title, "Test Paper");
    }

    #[test]
    fn test_find_by_doi() {
        let (_, repo) = setup();
        let mut paper = Paper::new("DOI Paper");
        paper.doi = Some("10.1234/test".to_string());
        repo.save(&paper).unwrap();

        let found = repo.find_by_doi("10.1234/test").unwrap().unwrap();
        assert_eq!(found.id, paper.id);
    }

    #[test]
    fn test_upsert_merges() {
        let (_, repo) = setup();
        let mut paper = Paper::new("Merge Test");
        paper.doi = Some("10.1234/merge".to_string());
        repo.save(&paper).unwrap();

        let mut updated = paper.clone();
        updated.arxiv_id = Some("2301.00001".to_string());
        repo.save(&updated).unwrap();

        let loaded = repo.get(paper.id.as_str()).unwrap().unwrap();
        assert_eq!(loaded.arxiv_id, Some("2301.00001".to_string()));
    }

    #[test]
    fn test_list_all() {
        let (_, repo) = setup();
        for i in 0..5 {
            let paper = Paper::new(format!("Paper {i}"));
            repo.save(&paper).unwrap();
        }

        let papers = repo.list_all(3, 0).unwrap();
        assert_eq!(papers.len(), 3);
    }
}
