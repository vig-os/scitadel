use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};

use scitadel_core::models::{
    Anchor, AnchorStatus, Annotation, Assessment, Paper, PaperId, ResearchQuestion, Search,
    SearchTerm,
};
use scitadel_core::ports::{
    AssessmentRepository, PaperRepository, QuestionRepository, SearchRepository,
};
use scitadel_db::sqlite::{Database, SqliteAnnotationRepository, SqlitePaperStateRepository};

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

    pub fn load_assessments_for_paper(
        &self,
        paper_id: &str,
        question_id: Option<&str>,
    ) -> Result<Vec<Assessment>> {
        let (_, _, _, a_repo, _) = self.db.repositories();
        Ok(a_repo.get_for_paper(paper_id, question_id)?)
    }

    /// Load the set of paper IDs this reader has starred.
    pub fn load_starred_ids(&self, reader: &str) -> Result<HashSet<String>> {
        Ok(SqlitePaperStateRepository::new(self.db.clone()).starred_ids(reader)?)
    }

    /// Load starred papers for this reader in most-recently-starred-first
    /// order, fully hydrated. Drives the Queue tab (#48) — a cross-
    /// search aggregator of starred papers across all questions.
    pub fn load_starred_papers(&self, reader: &str) -> Result<Vec<Paper>> {
        let ids = SqlitePaperStateRepository::new(self.db.clone()).starred_ids_ordered(reader)?;
        let (paper_repo, _, _, _, _) = self.db.repositories();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(Some(p)) = paper_repo.get(&id) {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Load live annotations for a paper (roots + replies), ordered oldest-first.
    pub fn load_annotations_for_paper(&self, paper_id: &str) -> Result<Vec<Annotation>> {
        Ok(SqliteAnnotationRepository::new(self.db.clone()).list_by_paper(paper_id)?)
    }

    /// Toggle the starred flag for a paper and return the new value.
    pub fn toggle_starred(&self, paper_id: &str, reader: &str) -> Result<bool> {
        Ok(SqlitePaperStateRepository::new(self.db.clone()).toggle_starred(paper_id, reader)?)
    }

    /// Create a root annotation anchored to a quoted passage. Anchors
    /// from the TUI carry only the quote (no offsets), so the resolver
    /// will substring-match on next paper-open.
    pub fn create_root_annotation(
        &self,
        paper_id: &str,
        quote: &str,
        note: &str,
        author: &str,
    ) -> Result<String> {
        let anchor = Anchor {
            quote: Some(quote.to_string()),
            status: AnchorStatus::Ok,
            ..Anchor::default()
        };
        let ann = Annotation::new_root(
            PaperId::from(paper_id),
            author.to_string(),
            note.to_string(),
            anchor,
        );
        SqliteAnnotationRepository::new(self.db.clone()).create(&ann)?;
        Ok(ann.id.as_str().to_string())
    }

    /// Reply to an existing annotation; inherits paper_id + question_id
    /// from the parent.
    pub fn reply_annotation(&self, parent_id: &str, note: &str, author: &str) -> Result<String> {
        let repo = SqliteAnnotationRepository::new(self.db.clone());
        let parent = repo
            .get(parent_id)?
            .with_context(|| format!("annotation {parent_id} not found"))?;
        let reply = Annotation::new_reply(&parent, author.to_string(), note.to_string());
        repo.create(&reply)?;
        Ok(reply.id.as_str().to_string())
    }

    /// Update the note body of an existing annotation; preserves color +
    /// tags as-is.
    pub fn update_annotation_note(&self, annotation_id: &str, note: &str) -> Result<()> {
        let repo = SqliteAnnotationRepository::new(self.db.clone());
        let existing = repo
            .get(annotation_id)?
            .with_context(|| format!("annotation {annotation_id} not found"))?;
        repo.update_note(
            annotation_id,
            note,
            existing.color.as_deref(),
            &existing.tags,
        )?;
        Ok(())
    }

    /// Soft-delete an annotation (tombstone — replies stay readable).
    pub fn delete_annotation(&self, annotation_id: &str) -> Result<()> {
        Ok(SqliteAnnotationRepository::new(self.db.clone()).soft_delete(annotation_id)?)
    }

    /// Persist the outcome of a download attempt for the Papers-table
    /// state column (#112). `local_path` is the absolute path to the
    /// saved file on success, `None` on failure.
    pub fn record_download_outcome(
        &self,
        paper_id: &str,
        local_path: Option<&str>,
        status: scitadel_core::models::DownloadStatus,
    ) -> Result<()> {
        let (paper_repo, _, _, _, _) = self.db.repositories();
        Ok(paper_repo.update_download_state(paper_id, local_path, status)?)
    }

    /// Persist the TUI's current selection so an MCP-side agent can
    /// query it (#122).
    pub fn publish_tui_state(&self, state: &scitadel_db::sqlite::TuiState) -> Result<()> {
        Ok(scitadel_db::sqlite::SqliteTuiStateRepository::new(self.db.clone()).set(state)?)
    }
}
