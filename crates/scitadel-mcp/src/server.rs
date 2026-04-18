use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool,
};

use crate::tools;

// ---------- Aggregate request structs ----------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchRequest {
    #[schemars(description = "Search query string")]
    pub query: String,
    #[schemars(
        description = "Comma-separated list of sources (e.g. pubmed,arxiv,openalex,inspire)"
    )]
    pub sources: String,
    #[schemars(description = "Maximum results per source")]
    pub max_results: usize,
    #[schemars(description = "Optional research question ID to link the search")]
    pub question_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddSearchTermsRequest {
    #[schemars(description = "Research question ID")]
    pub question_id: String,
    #[schemars(description = "List of search terms")]
    pub terms: Vec<String>,
    #[schemars(description = "Custom query string (optional, defaults to terms joined by space)")]
    pub query_string: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AssessPaperRequest {
    #[schemars(description = "Paper ID")]
    pub paper_id: String,
    #[schemars(description = "Research question ID")]
    pub question_id: String,
    #[schemars(description = "Relevance score (0.0-1.0)")]
    pub score: f64,
    #[schemars(description = "Reasoning for the score")]
    pub reasoning: String,
    #[schemars(description = "Assessor identifier")]
    pub assessor: String,
    #[schemars(description = "Model used for assessment (optional)")]
    pub model: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreateAnnotationRequest {
    #[schemars(description = "Paper ID the annotation anchors to")]
    pub paper_id: String,
    #[schemars(description = "Exact quoted passage (TextQuoteSelector body)")]
    pub quote: String,
    #[schemars(description = "Note body — markdown allowed")]
    pub note: String,
    #[schemars(description = "Identity of the author (e.g. lars, claude-opus-4-7)")]
    pub author: String,
    #[schemars(description = "Text immediately before the quote, for anchor disambiguation")]
    pub prefix: Option<String>,
    #[schemars(description = "Text immediately after the quote")]
    pub suffix: Option<String>,
    #[schemars(description = "Optional research-question ID to link the annotation")]
    pub question_id: Option<String>,
    #[schemars(description = "Optional color label (hex or name)")]
    pub color: Option<String>,
    #[schemars(description = "Optional tag list")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateAnnotationRequest {
    #[schemars(description = "Annotation ID")]
    pub id: String,
    #[schemars(description = "New note body")]
    pub note: Option<String>,
    #[schemars(description = "New color")]
    pub color: Option<String>,
    #[schemars(description = "Replace tag list wholesale")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SaveAssessmentRequest {
    #[schemars(description = "Paper ID")]
    pub paper_id: String,
    #[schemars(description = "Research question ID")]
    pub question_id: String,
    #[schemars(description = "Relevance score (0.0-1.0)")]
    pub score: f64,
    #[schemars(description = "Reasoning for the score")]
    pub reasoning: String,
}

// ---------- Server ----------

#[derive(Debug, Clone, Default)]
pub struct ScitadelServer;

#[tool(tool_box)]
impl ScitadelServer {
    #[tool(description = "Search scientific literature across multiple sources")]
    async fn search(&self, #[tool(aggr)] req: SearchRequest) -> Result<String, String> {
        tools::search_tool(req.query, req.sources, req.max_results, req.question_id).await
    }

    #[tool(
        description = "List every source scitadel knows about (pubmed, arxiv, openalex, inspire, patentsview, lens, epo) with per-source description, required credential fields, whether credentials are configured in this environment, and rate-limit hints. Read-only; call first to decide which sources to pass to `search`."
    )]
    fn list_sources(&self) -> Result<String, String> {
        tools::list_sources_tool()
    }

    #[tool(
        description = "Return the scoring rubric (criteria, 0.0-1.0 scale, response format) as a string. Fetch once at the start of a scoring session and cache; use with `save_assessment` or `assess_paper` for each paper. Avoids the per-paper rubric fetch that `prepare_assessment` does."
    )]
    fn get_rubric(&self) -> Result<String, String> {
        tools::get_rubric_tool()
    }

    #[tool(
        description = "Create an annotation anchored to a passage in a paper. Root-level; use `reply_annotation` for replies. `author` is required — pass your identity string (e.g. agent slug)."
    )]
    fn create_annotation(
        &self,
        #[tool(aggr)] req: CreateAnnotationRequest,
    ) -> Result<String, String> {
        tools::create_annotation_tool(
            &req.paper_id,
            &req.quote,
            &req.note,
            &req.author,
            req.prefix.as_deref(),
            req.suffix.as_deref(),
            req.question_id.as_deref(),
            req.color.as_deref(),
            req.tags,
        )
    }

    #[tool(
        description = "Reply to an existing annotation. Inherits paper_id + question_id from the parent; the reply has no anchor of its own."
    )]
    fn reply_annotation(
        &self,
        #[tool(param)]
        #[schemars(description = "Parent annotation ID")]
        parent_id: String,
        #[tool(param)]
        #[schemars(description = "Reply body")]
        note: String,
        #[tool(param)]
        #[schemars(description = "Author identity")]
        author: String,
    ) -> Result<String, String> {
        tools::reply_annotation_tool(&parent_id, &note, &author)
    }

    #[tool(description = "Update note / color / tags on an existing annotation.")]
    fn update_annotation(
        &self,
        #[tool(aggr)] req: UpdateAnnotationRequest,
    ) -> Result<String, String> {
        tools::update_annotation_tool(&req.id, req.note.as_deref(), req.color.as_deref(), req.tags)
    }

    #[tool(
        description = "Soft-delete an annotation (tombstone). Threads stay intact; list_annotations hides the row."
    )]
    fn delete_annotation(
        &self,
        #[tool(param)]
        #[schemars(description = "Annotation ID")]
        id: String,
    ) -> Result<String, String> {
        tools::delete_annotation_tool(&id)
    }

    #[tool(
        description = "List annotations for a paper (required). Optional author filter. Returns JSON array with id, parent_id, anchor, note, tags, author, timestamps, and anchor_status."
    )]
    fn list_annotations(
        &self,
        #[tool(param)]
        #[schemars(description = "Paper ID to list annotations for")]
        paper_id: String,
        #[tool(param)]
        #[schemars(description = "Optional — only annotations by this author")]
        author: Option<String>,
    ) -> Result<String, String> {
        tools::list_annotations_tool(Some(&paper_id), author.as_deref())
    }

    #[tool(
        description = "Mark one or more annotations as seen by `reader`. Repeat calls just update seen_at. Used so an agent can stop re-processing notes it already handled."
    )]
    fn mark_seen(
        &self,
        #[tool(param)]
        #[schemars(description = "Annotation IDs to mark seen")]
        annotation_ids: Vec<String>,
        #[tool(param)]
        #[schemars(description = "Reader identity (e.g. agent slug)")]
        reader: String,
    ) -> Result<String, String> {
        tools::mark_seen_tool(annotation_ids, &reader)
    }

    #[tool(
        description = "Mark a whole annotation thread (root + replies) as seen by `reader` in one call."
    )]
    fn mark_thread_seen(
        &self,
        #[tool(param)]
        #[schemars(description = "Root annotation ID")]
        root_id: String,
        #[tool(param)]
        #[schemars(description = "Reader identity")]
        reader: String,
    ) -> Result<String, String> {
        tools::mark_thread_seen_tool(&root_id, &reader)
    }

    #[tool(
        description = "List annotations `reader` has not yet seen (or that were edited since last seen). Optional paper_id scopes the query. Use at session start to pick up human replies from the previous turn."
    )]
    fn list_unread(
        &self,
        #[tool(param)]
        #[schemars(description = "Reader identity")]
        reader: String,
        #[tool(param)]
        #[schemars(description = "Optional paper ID filter")]
        paper_id: Option<String>,
    ) -> Result<String, String> {
        tools::list_unread_tool(&reader, paper_id.as_deref())
    }

    #[tool(
        description = "Full-text search over stored past searches (FTS5 + Porter stemming). Returns JSON array of matching prior searches sorted by relevance (lower rank = more relevant). Call before running a fresh `search` to detect redundant work."
    )]
    fn find_similar_searches(
        &self,
        #[tool(param)]
        #[schemars(description = "Free-text query — FTS5 operators are stripped automatically")]
        query: String,
        #[tool(param)]
        #[schemars(description = "Max hits to return (default 10)")]
        limit: Option<i64>,
    ) -> Result<String, String> {
        tools::find_similar_searches_tool(&query, limit)
    }

    #[tool(
        description = "Summarize every paper in a search as JSON in one call: title, authors, year, abstract (truncated), DOI, identifiers. Preferred over iterating `get_paper` per result when scanning a corpus."
    )]
    fn summarize_search(
        &self,
        #[tool(param)]
        #[schemars(description = "Search ID")]
        search_id: String,
        #[tool(param)]
        #[schemars(description = "Max papers to return (default 50)")]
        max_papers: Option<usize>,
        #[tool(param)]
        #[schemars(description = "Max chars per abstract before truncation (default 500)")]
        abstract_char_limit: Option<usize>,
    ) -> Result<String, String> {
        tools::summarize_search_tool(&search_id, max_papers, abstract_char_limit)
    }

    #[tool(description = "List recent search runs")]
    fn list_searches(
        &self,
        #[tool(param)]
        #[schemars(description = "Maximum number of searches to return")]
        limit: Option<i64>,
    ) -> Result<String, String> {
        tools::list_searches_tool(limit.unwrap_or(20))
    }

    #[tool(description = "Get papers from a search result")]
    fn get_papers(
        &self,
        #[tool(param)]
        #[schemars(description = "Search ID")]
        search_id: String,
    ) -> Result<String, String> {
        tools::get_papers_tool(&search_id)
    }

    #[tool(description = "Get full details of a single paper")]
    fn get_paper(
        &self,
        #[tool(param)]
        #[schemars(description = "Paper ID")]
        paper_id: String,
    ) -> Result<String, String> {
        tools::get_paper_tool(&paper_id)
    }

    #[tool(description = "Export search results in a given format (json, csv, bibtex)")]
    fn export_search(
        &self,
        #[tool(param)]
        #[schemars(description = "Search ID")]
        search_id: String,
        #[tool(param)]
        #[schemars(description = "Export format: json, csv, or bibtex")]
        format: String,
    ) -> Result<String, String> {
        tools::export_search_tool(&search_id, &format)
    }

    #[tool(description = "Create a new research question")]
    fn create_question(
        &self,
        #[tool(param)]
        #[schemars(description = "Question text")]
        text: String,
        #[tool(param)]
        #[schemars(description = "Additional context or description")]
        description: String,
    ) -> Result<String, String> {
        tools::create_question_tool(&text, &description)
    }

    #[tool(description = "List all research questions")]
    fn list_questions(&self) -> Result<String, String> {
        tools::list_questions_tool()
    }

    #[tool(description = "Add search terms linked to a research question")]
    fn add_search_terms(&self, #[tool(aggr)] req: AddSearchTermsRequest) -> Result<String, String> {
        tools::add_search_terms_tool(&req.question_id, &req.terms, &req.query_string)
    }

    #[tool(description = "Record a paper assessment with score and reasoning")]
    fn assess_paper(&self, #[tool(aggr)] req: AssessPaperRequest) -> Result<String, String> {
        tools::assess_paper_tool(
            &req.paper_id,
            &req.question_id,
            req.score,
            &req.reasoning,
            &req.assessor,
            req.model.as_deref(),
        )
    }

    #[tool(description = "Get assessments for a paper and/or question")]
    fn get_assessments(
        &self,
        #[tool(param)]
        #[schemars(description = "Paper ID (optional)")]
        paper_id: Option<String>,
        #[tool(param)]
        #[schemars(description = "Research question ID (optional)")]
        question_id: Option<String>,
    ) -> Result<String, String> {
        tools::get_assessments_tool(paper_id.as_deref(), question_id.as_deref())
    }

    #[tool(description = "Prepare assessment rubric and paper data for LLM evaluation")]
    fn prepare_assessment(
        &self,
        #[tool(param)]
        #[schemars(description = "Paper ID")]
        paper_id: String,
        #[tool(param)]
        #[schemars(description = "Research question ID")]
        question_id: String,
    ) -> Result<String, String> {
        tools::prepare_assessment_tool(&paper_id, &question_id)
    }

    #[tool(description = "Save an MCP-native assessment scored by the host LLM")]
    fn save_assessment(&self, #[tool(aggr)] req: SaveAssessmentRequest) -> Result<String, String> {
        tools::save_assessment_tool(&req.paper_id, &req.question_id, req.score, &req.reasoning)
    }

    #[tool(
        description = "Download a paper (PDF or HTML). Prefer passing paper_id to leverage all stored identifiers (arxiv/openalex/doi); doi is a fallback for ad-hoc lookups."
    )]
    async fn download_paper(
        &self,
        #[tool(param)]
        #[schemars(
            description = "Paper ID from the scitadel DB (preferred — unlocks arxiv/openalex/Unpaywall chain)"
        )]
        paper_id: Option<String>,
        #[tool(param)]
        #[schemars(description = "DOI (used only if paper_id is not provided)")]
        doi: Option<String>,
        #[tool(param)]
        #[schemars(description = "Output directory (optional, defaults to .scitadel/papers/)")]
        output_dir: Option<String>,
    ) -> Result<String, String> {
        tools::download_paper_tool(paper_id.as_deref(), doi.as_deref(), output_dir.as_deref()).await
    }

    #[tool(
        description = "Extract the text from an already-downloaded paper's PDF or HTML. Call download_paper first."
    )]
    async fn read_paper(
        &self,
        #[tool(param)]
        #[schemars(description = "Paper ID")]
        paper_id: String,
        #[tool(param)]
        #[schemars(description = "Max characters to return (default 20000)")]
        max_chars: Option<usize>,
    ) -> Result<String, String> {
        tools::read_paper_tool(&paper_id, max_chars).await
    }

    #[tool(description = "Prepare batch assessments for all papers in a search")]
    fn prepare_batch_assessments(
        &self,
        #[tool(param)]
        #[schemars(description = "Search ID")]
        search_id: String,
        #[tool(param)]
        #[schemars(description = "Research question ID")]
        question_id: String,
    ) -> Result<String, String> {
        tools::prepare_batch_assessments_tool(&search_id, &question_id)
    }
}

#[tool(tool_box)]
impl ServerHandler for ScitadelServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Scitadel: scientific literature retrieval and assessment".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
