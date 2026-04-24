use std::collections::HashMap;

use crate::error::CoreError;
use crate::models::{
    Assessment, Citation, DownloadStatus, Paper, PaperId, ResearchQuestion, Search, SearchResult,
    SearchTerm, SnowballRun,
};

/// Port for paper persistence.
pub trait PaperRepository: Send + Sync {
    fn save(&self, paper: &Paper) -> Result<(), CoreError>;
    /// Save multiple papers, resolving DOI conflicts with existing records.
    /// Returns a map of original_id → resolved_id for papers whose ID was
    /// remapped to an existing record (callers should use this to fix up
    /// search_results and other references).
    fn save_many(&self, papers: &[Paper]) -> Result<HashMap<PaperId, PaperId>, CoreError>;
    fn get(&self, paper_id: &str) -> Result<Option<Paper>, CoreError>;
    fn find_by_doi(&self, doi: &str) -> Result<Option<Paper>, CoreError>;
    fn find_by_title(&self, title: &str) -> Result<Option<Paper>, CoreError>;
    fn list_all(&self, limit: i64, offset: i64) -> Result<Vec<Paper>, CoreError>;
    /// Persist extracted full text for a paper. Used by `read_paper_tool`
    /// after the first PDF/HTML extraction so subsequent reads (TUI
    /// reader, MCP `get_annotated_paper`) hit the DB instead of
    /// re-running pdf-extract. Idempotent — overwrites the existing
    /// `full_text` column.
    fn update_full_text(&self, paper_id: &str, text: &str) -> Result<(), CoreError>;
    /// Persist the outcome of a download attempt. `local_path` is the
    /// absolute path to the saved file on success (any non-`Failed`
    /// status); pass `None` for `Failed`. `last_attempt_at` is set to
    /// `now()` automatically. See #112.
    fn update_download_state(
        &self,
        paper_id: &str,
        local_path: Option<&str>,
        status: DownloadStatus,
    ) -> Result<(), CoreError>;
    /// Assign a stable citation key to a paper. Called once per paper
    /// by the 0.6.0 backfill migration (and per-paper on new ingest).
    /// Must be unique across the DB — the caller is responsible for
    /// running the Better-BibTeX algorithm + disambiguation before
    /// calling this. See ADR-006 + `scitadel-export::bibtex::assign_keys`.
    fn update_bibtex_key(&self, paper_id: &str, key: &str) -> Result<(), CoreError>;
}

/// Port for search run persistence.
pub trait SearchRepository: Send + Sync {
    fn save(&self, search: &Search) -> Result<(), CoreError>;
    fn get(&self, search_id: &str) -> Result<Option<Search>, CoreError>;
    fn save_results(&self, results: &[SearchResult]) -> Result<(), CoreError>;
    fn get_results(&self, search_id: &str) -> Result<Vec<SearchResult>, CoreError>;
    fn list_searches(&self, limit: i64) -> Result<Vec<Search>, CoreError>;
    fn diff_searches(
        &self,
        search_id_a: &str,
        search_id_b: &str,
    ) -> Result<(Vec<String>, Vec<String>), CoreError>;
}

/// Port for research question and search term persistence.
pub trait QuestionRepository: Send + Sync {
    fn save_question(&self, question: &ResearchQuestion) -> Result<(), CoreError>;
    fn get_question(&self, question_id: &str) -> Result<Option<ResearchQuestion>, CoreError>;
    fn list_questions(&self) -> Result<Vec<ResearchQuestion>, CoreError>;
    fn save_term(&self, term: &SearchTerm) -> Result<(), CoreError>;
    fn get_terms(&self, question_id: &str) -> Result<Vec<SearchTerm>, CoreError>;
}

/// Port for relevance assessment persistence.
pub trait AssessmentRepository: Send + Sync {
    fn save(&self, assessment: &Assessment) -> Result<(), CoreError>;
    fn get_for_paper(
        &self,
        paper_id: &str,
        question_id: Option<&str>,
    ) -> Result<Vec<Assessment>, CoreError>;
    fn get_for_question(&self, question_id: &str) -> Result<Vec<Assessment>, CoreError>;
}

/// Port for citation edge and snowball run persistence.
pub trait CitationRepository: Send + Sync {
    fn save(&self, citation: &Citation) -> Result<(), CoreError>;
    fn save_many(&self, citations: &[Citation]) -> Result<(), CoreError>;
    fn get_references(&self, paper_id: &str) -> Result<Vec<Citation>, CoreError>;
    fn get_citations(&self, paper_id: &str) -> Result<Vec<Citation>, CoreError>;
    fn exists(
        &self,
        source_paper_id: &str,
        target_paper_id: &str,
        direction: &str,
    ) -> Result<bool, CoreError>;
    fn save_snowball_run(&self, run: &SnowballRun) -> Result<(), CoreError>;
    fn get_snowball_run(&self, run_id: &str) -> Result<Option<SnowballRun>, CoreError>;
    fn list_snowball_runs(&self, limit: i64) -> Result<Vec<SnowballRun>, CoreError>;
}
