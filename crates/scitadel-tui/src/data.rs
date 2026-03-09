use std::path::Path;

use anyhow::{Context, Result};

use scitadel_core::models::{Assessment, Paper, ResearchQuestion, Search, SearchTerm};
use scitadel_core::ports::{
    AssessmentRepository, PaperRepository, QuestionRepository, SearchRepository,
};
use scitadel_db::sqlite::Database;

/// Wrapper around the database that loads data for each TUI view.
pub struct DataStore {
    db: Database,
}

impl DataStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        let db = Database::open(db_path).context("failed to open database")?;
        db.migrate().context("migration failed")?;
        Ok(Self { db })
    }

    pub fn load_searches(&self, limit: i64) -> Result<Vec<Search>> {
        let (_, search_repo, _, _, _) = self.db.repositories();
        Ok(search_repo.list_searches(limit)?)
    }

    pub fn load_papers(&self, limit: i64, offset: i64) -> Result<Vec<Paper>> {
        let (paper_repo, _, _, _, _) = self.db.repositories();
        Ok(paper_repo.list_all(limit, offset)?)
    }

    pub fn load_paper(&self, paper_id: &str) -> Result<Option<Paper>> {
        let (paper_repo, _, _, _, _) = self.db.repositories();
        Ok(paper_repo.get(paper_id)?)
    }

    pub fn load_papers_for_search(&self, search_id: &str) -> Result<Vec<Paper>> {
        let (paper_repo, search_repo, _, _, _) = self.db.repositories();
        let results = search_repo.get_results(search_id)?;
        let mut papers = Vec::new();
        for r in &results {
            if let Ok(Some(paper)) = paper_repo.get(r.paper_id.as_str()) {
                papers.push(paper);
            }
        }
        Ok(papers)
    }

    pub fn load_questions(&self) -> Result<Vec<ResearchQuestion>> {
        let (_, _, q_repo, _, _) = self.db.repositories();
        Ok(q_repo.list_questions()?)
    }

    pub fn load_terms(&self, question_id: &str) -> Result<Vec<SearchTerm>> {
        let (_, _, q_repo, _, _) = self.db.repositories();
        Ok(q_repo.get_terms(question_id)?)
    }

    pub fn load_searches_for_question(&self, question_id: &str) -> Result<Vec<Search>> {
        let searches = self.load_searches(100)?;
        Ok(searches
            .into_iter()
            .filter(|s| {
                s.parameters
                    .get("question_id")
                    .and_then(|v| v.as_str())
                    .is_some_and(|qid| qid == question_id)
            })
            .collect())
    }

    pub fn load_assessments_for_paper(
        &self,
        paper_id: &str,
        question_id: Option<&str>,
    ) -> Result<Vec<Assessment>> {
        let (_, _, _, a_repo, _) = self.db.repositories();
        Ok(a_repo.get_for_paper(paper_id, question_id)?)
    }
}
