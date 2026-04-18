use std::collections::HashMap;

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

    /// If a paper with the same DOI already exists, return a clone with the existing ID
    /// so the upsert merges into the existing row instead of violating the DOI unique index.
    fn resolve_doi_conflict(
        conn: &rusqlite::Connection,
        paper: &Paper,
    ) -> Result<Paper, CoreError> {
        if let Some(doi) = &paper.doi {
            let existing_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM papers WHERE doi = ?1 AND id != ?2",
                    params![doi, paper.id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(DbError::Sqlite)?;
            if let Some(id) = existing_id {
                let mut merged = paper.clone();
                merged.id = PaperId::from(id);
                return Ok(merged);
            }
        }
        Ok(paper.clone())
    }

    fn resolve_doi_conflict_tx(
        tx: &rusqlite::Transaction<'_>,
        paper: &Paper,
    ) -> Result<Paper, CoreError> {
        if let Some(doi) = &paper.doi {
            let existing_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM papers WHERE doi = ?1 AND id != ?2",
                    params![doi, paper.id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(DbError::Sqlite)?;
            if let Some(id) = existing_id {
                let mut merged = paper.clone();
                merged.id = PaperId::from(id);
                return Ok(merged);
            }
        }
        Ok(paper.clone())
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
        let paper = Self::resolve_doi_conflict(&conn, paper)?;
        let p = Self::paper_params(&paper);
        let params: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        match conn.execute(UPSERT_SQL, params.as_slice()) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                // DOI collision — retry with existing paper's ID
                if let Some(doi) = &paper.doi {
                    let existing_id: Option<String> = conn
                        .query_row(
                            "SELECT id FROM papers WHERE doi = ?1",
                            params![doi],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(DbError::Sqlite)?;
                    if let Some(eid) = existing_id {
                        let mut retry = paper.clone();
                        retry.id = PaperId::from(eid);
                        let p2 = Self::paper_params(&retry);
                        let params2: Vec<&dyn rusqlite::types::ToSql> =
                            p2.iter().map(|b| b.as_ref()).collect();
                        conn.execute(UPSERT_SQL, params2.as_slice())
                            .map_err(DbError::Sqlite)?;
                    }
                }
                Ok(())
            }
            Err(e) => Err(DbError::Sqlite(e).into()),
        }
    }

    fn save_many(&self, papers: &[Paper]) -> Result<HashMap<PaperId, PaperId>, CoreError> {
        let mut conn = self.db.conn()?;
        let mut id_remap = HashMap::new();
        let tx = conn.transaction().map_err(DbError::Sqlite)?;
        for paper in papers {
            let resolved = Self::resolve_doi_conflict_tx(&tx, paper)?;
            if resolved.id != paper.id {
                id_remap.insert(paper.id.clone(), resolved.id.clone());
            }
            let p = Self::paper_params(&resolved);
            let params: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|b| b.as_ref()).collect();
            match tx.execute(UPSERT_SQL, params.as_slice()) {
                Ok(_) => {}
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    // DOI unique-index collision that resolve_doi_conflict missed
                    // (e.g. case variation, concurrent insert, or within-batch dup).
                    // Look up the existing paper by DOI and retry as an update.
                    if let Some(doi) = &resolved.doi {
                        let existing_id: Option<String> = tx
                            .query_row(
                                "SELECT id FROM papers WHERE doi = ?1",
                                params![doi],
                                |row| row.get(0),
                            )
                            .optional()
                            .map_err(DbError::Sqlite)?;
                        if let Some(eid) = existing_id {
                            id_remap.insert(paper.id.clone(), PaperId::from(eid.clone()));
                            let mut retry = resolved.clone();
                            retry.id = PaperId::from(eid);
                            let p2 = Self::paper_params(&retry);
                            let params2: Vec<&dyn rusqlite::types::ToSql> =
                                p2.iter().map(|b| b.as_ref()).collect();
                            tx.execute(UPSERT_SQL, params2.as_slice())
                                .map_err(DbError::Sqlite)?;
                        }
                        // If no DOI match found either, skip silently — paper
                        // may have been blocked by another unique constraint.
                    }
                }
                Err(e) => return Err(DbError::Sqlite(e).into()),
            }
        }
        tx.commit().map_err(DbError::Sqlite)?;
        Ok(id_remap)
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
    fn test_doi_conflict_across_papers() {
        let (_, repo) = setup();

        // First paper with a DOI
        let mut paper1 = Paper::new("Original Paper");
        paper1.doi = Some("10.1234/conflict".to_string());
        repo.save(&paper1).unwrap();

        // Second paper with same DOI but different ID (simulates a second search)
        let mut paper2 = Paper::new("Updated Title");
        paper2.doi = Some("10.1234/conflict".to_string());
        paper2.arxiv_id = Some("2301.99999".to_string());
        repo.save(&paper2).unwrap();

        // Should have merged into the original, not created a second row
        let loaded = repo.find_by_doi("10.1234/conflict").unwrap().unwrap();
        assert_eq!(loaded.id, paper1.id, "should reuse original paper ID");
        assert_eq!(loaded.title, "Updated Title", "should update title");
        assert_eq!(
            loaded.arxiv_id,
            Some("2301.99999".to_string()),
            "should merge arxiv_id"
        );
    }

    #[test]
    fn test_doi_conflict_in_save_many() {
        let (_, repo) = setup();

        let mut existing = Paper::new("Existing Paper");
        existing.doi = Some("10.1234/batch".to_string());
        repo.save(&existing).unwrap();

        // Batch save with a colliding DOI
        let mut new_paper = Paper::new("Batch Paper");
        new_paper.doi = Some("10.1234/batch".to_string());
        new_paper.pubmed_id = Some("12345".to_string());
        repo.save_many(&[new_paper]).unwrap();

        let loaded = repo.find_by_doi("10.1234/batch").unwrap().unwrap();
        assert_eq!(loaded.id, existing.id);
        assert_eq!(loaded.pubmed_id, Some("12345".to_string()));
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

    /// Integration test: simulate two searches with overlapping DOIs going
    /// through dedup → save_many, the same flow as the MCP search tool.
    #[test]
    fn test_cross_search_dedup_save_roundtrip() {
        use scitadel_core::models::CandidatePaper;
        use scitadel_core::services::dedup::deduplicate;

        let (_, repo) = setup();

        // --- First search: returns 3 papers ---
        let candidates_1 = vec![
            CandidatePaper {
                doi: Some("10.1000/alpha".into()),
                ..CandidatePaper::new("openalex", "oa-1", "Alpha Paper")
            },
            CandidatePaper {
                doi: Some("10.1000/beta".into()),
                ..CandidatePaper::new("openalex", "oa-2", "Beta Paper")
            },
            CandidatePaper {
                doi: Some("10.1000/gamma".into()),
                ..CandidatePaper::new("pubmed", "pm-1", "Gamma Paper")
            },
        ];
        let (papers_1, _results_1) = deduplicate(&candidates_1, 0.85);
        assert_eq!(papers_1.len(), 3);
        let remap_1 = repo.save_many(&papers_1).unwrap();
        assert!(remap_1.is_empty(), "no conflicts on first save");

        // --- Second search: 2 overlapping DOIs + 1 new ---
        let candidates_2 = vec![
            CandidatePaper {
                doi: Some("10.1000/alpha".into()),
                arxiv_id: Some("2301.00001".into()),
                ..CandidatePaper::new("arxiv", "ax-1", "Alpha Paper (arxiv)")
            },
            CandidatePaper {
                doi: Some("10.1000/gamma".into()),
                pubmed_id: Some("99999".into()),
                ..CandidatePaper::new("pubmed", "pm-2", "Gamma Paper Revised")
            },
            CandidatePaper {
                doi: Some("10.1000/delta".into()),
                ..CandidatePaper::new("openalex", "oa-3", "Delta Paper")
            },
        ];
        let (papers_2, results_2) = deduplicate(&candidates_2, 0.85);
        assert_eq!(
            papers_2.len(),
            3,
            "dedup sees them as distinct (different IDs)"
        );

        let remap_2 = repo.save_many(&papers_2).unwrap();
        assert_eq!(
            remap_2.len(),
            2,
            "alpha and gamma should remap to existing IDs"
        );

        // Verify the remap points to the original paper IDs
        let alpha_original = papers_1
            .iter()
            .find(|p| p.doi.as_deref() == Some("10.1000/alpha"))
            .unwrap();
        let alpha_new = papers_2
            .iter()
            .find(|p| p.doi.as_deref() == Some("10.1000/alpha"))
            .unwrap();
        assert_eq!(remap_2[&alpha_new.id], alpha_original.id);

        // Verify DB state: should have 4 papers total, not 6
        let all = repo.list_all(100, 0).unwrap();
        assert_eq!(all.len(), 4, "3 from first search + 1 new from second");

        // Verify metadata was merged
        let alpha = repo.find_by_doi("10.1000/alpha").unwrap().unwrap();
        assert_eq!(alpha.id, alpha_original.id, "kept original ID");
        assert_eq!(
            alpha.arxiv_id,
            Some("2301.00001".into()),
            "merged arxiv_id from second search"
        );

        // Verify search_results can be remapped correctly
        for sr in &results_2 {
            let resolved_id = remap_2.get(&sr.paper_id).unwrap_or(&sr.paper_id);
            assert!(
                repo.get(resolved_id.as_str()).unwrap().is_some(),
                "remapped paper_id should exist in DB"
            );
        }
    }
}
