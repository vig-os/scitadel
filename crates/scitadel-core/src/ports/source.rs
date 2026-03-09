use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::CandidatePaper;

/// Port for external source adapters (`PubMed`, `arXiv`, `OpenAlex`, `INSPIRE`).
#[async_trait]
pub trait SourceAdapter: Send + Sync {
    /// Human-readable name of this source (e.g., "pubmed", "arxiv").
    fn name(&self) -> &str;

    /// Search the source and return normalized candidate records.
    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError>;
}
