//! MCP server using rmcp 0.17's `tool_router` macro.
//!
//! Each tool is a method on `ScitadelServer` annotated with `#[tool]`.
//! Aggregate request structs implement `Deserialize + JsonSchema` and
//! are extracted via `Parameters<T>`. The tool handler functions live
//! in `crate::tools` so this file stays a thin façade.

use rmcp::{
    Peer, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ProgressNotificationParam, ProgressToken},
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools;

/// Helper: emit a progress notification if the caller opted in by
/// supplying a `progressToken` in the request `_meta`. Failures are
/// logged but never propagated — progress is best-effort.
async fn notify(
    peer: &Peer<RoleServer>,
    token: Option<&ProgressToken>,
    progress: f64,
    total: Option<f64>,
    message: impl Into<String>,
) {
    let Some(token) = token else { return };
    let result = peer
        .notify_progress(ProgressNotificationParam {
            progress_token: token.clone(),
            progress,
            total,
            message: Some(message.into()),
        })
        .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to send progress notification");
    }
}

// ---------- Aggregate request structs ----------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchRequest {
    /// Search query string
    pub query: String,
    /// Comma-separated list of sources (e.g. pubmed,arxiv,openalex,inspire)
    pub sources: String,
    /// Maximum results per source
    pub max_results: usize,
    /// Optional research question ID to link the search
    pub question_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddSearchTermsRequest {
    /// Research question ID
    pub question_id: String,
    /// List of search terms
    pub terms: Vec<String>,
    /// Custom query string (optional, defaults to terms joined by space)
    pub query_string: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AssessPaperRequest {
    /// Paper ID
    pub paper_id: String,
    /// Research question ID
    pub question_id: String,
    /// Relevance score (0.0-1.0)
    pub score: f64,
    /// Reasoning for the score
    pub reasoning: String,
    /// Assessor identifier
    pub assessor: String,
    /// Model used for assessment (optional)
    pub model: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateAnnotationRequest {
    /// Paper ID the annotation anchors to
    pub paper_id: String,
    /// Exact quoted passage (TextQuoteSelector body)
    pub quote: String,
    /// Note body — markdown allowed
    pub note: String,
    /// Identity of the author (e.g. lars, claude-opus-4-7)
    pub author: String,
    /// Text immediately before the quote, for anchor disambiguation
    pub prefix: Option<String>,
    /// Text immediately after the quote
    pub suffix: Option<String>,
    /// Optional research-question ID to link the annotation
    pub question_id: Option<String>,
    /// Optional color label (hex or name)
    pub color: Option<String>,
    /// Optional tag list
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateAnnotationRequest {
    /// Annotation ID
    pub id: String,
    /// New note body
    pub note: Option<String>,
    /// New color
    pub color: Option<String>,
    /// Replace tag list wholesale
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveAssessmentRequest {
    /// Paper ID
    pub paper_id: String,
    /// Research question ID
    pub question_id: String,
    /// Relevance score (0.0-1.0)
    pub score: f64,
    /// Reasoning for the score
    pub reasoning: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplyAnnotationRequest {
    /// Parent annotation ID
    pub parent_id: String,
    /// Reply body
    pub note: String,
    /// Author identity
    pub author: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteAnnotationRequest {
    /// Annotation ID
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAnnotationsRequest {
    /// Paper ID to list annotations for
    pub paper_id: String,
    /// Optional — only annotations by this author
    pub author: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkSeenRequest {
    /// Annotation IDs to mark seen
    pub annotation_ids: Vec<String>,
    /// Reader identity (e.g. agent slug)
    pub reader: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkThreadSeenRequest {
    /// Root annotation ID
    pub root_id: String,
    /// Reader identity
    pub reader: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListUnreadRequest {
    /// Reader identity
    pub reader: String,
    /// Optional paper ID filter
    pub paper_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindSimilarSearchesRequest {
    /// Free-text query — FTS5 operators are stripped automatically
    pub query: String,
    /// Max hits to return (default 10)
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SummarizeSearchRequest {
    /// Search ID
    pub search_id: String,
    /// Max papers to return (default 50)
    pub max_papers: Option<usize>,
    /// Max chars per abstract before truncation (default 500)
    pub abstract_char_limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSearchesRequest {
    /// Maximum number of searches to return
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaperIdRequest {
    /// Paper ID
    pub paper_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCitationsRequest {
    /// Source paper ID (must have openalex_id)
    pub paper_id: String,
    /// Max citing works to return (default 25, max 200)
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportSearchRequest {
    /// Search ID
    pub search_id: String,
    /// Export format: json, csv, or bibtex
    pub format: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateQuestionRequest {
    /// Question text
    pub text: String,
    /// Additional context or description
    pub description: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetAssessmentsRequest {
    /// Paper ID (optional)
    pub paper_id: Option<String>,
    /// Research question ID (optional)
    pub question_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaperQuestionRequest {
    /// Paper ID
    pub paper_id: String,
    /// Research question ID
    pub question_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DownloadPaperRequest {
    /// Paper ID from the scitadel DB (preferred — unlocks arxiv/openalex/Unpaywall chain)
    pub paper_id: Option<String>,
    /// DOI (used only if paper_id is not provided)
    pub doi: Option<String>,
    /// Output directory (optional, defaults to .scitadel/papers/)
    pub output_dir: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadPaperRequest {
    /// Paper ID
    pub paper_id: String,
    /// Max characters to return (default 20000)
    pub max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchIdRequest {
    /// Search ID
    pub search_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchQuestionRequest {
    /// Search ID
    pub search_id: String,
    /// Research question ID
    pub question_id: String,
}

// ---------- Server ----------

#[derive(Debug, Clone)]
pub struct ScitadelServer {
    tool_router: ToolRouter<Self>,
}

impl ScitadelServer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for ScitadelServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = tool_router)]
impl ScitadelServer {
    #[tool(
        description = "Search scientific literature across multiple sources. Returns: JSON with search_id, query, per-source outcomes, total counts, and a `summary` text field for human readers. Emits MCP progress notifications (start + done) when the caller supplies a `progressToken` in `_meta` (#58)."
    )]
    async fn search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<String, String> {
        let token = ctx.meta.get_progress_token();
        let source_count = req
            .sources
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .count() as f64;
        notify(
            &ctx.peer,
            token.as_ref(),
            0.0,
            Some(source_count.max(1.0)),
            format!(
                "searching {} source(s) for \"{}\"",
                source_count.max(1.0),
                req.query
            ),
        )
        .await;
        let result =
            tools::search_tool(req.query, req.sources, req.max_results, req.question_id).await;
        let done_msg = match &result {
            Ok(_) => "search complete".to_string(),
            Err(e) => format!("search failed: {e}"),
        };
        notify(
            &ctx.peer,
            token.as_ref(),
            source_count.max(1.0),
            Some(source_count.max(1.0)),
            done_msg,
        )
        .await;
        result
    }

    #[tool(
        description = "List every source scitadel knows about (pubmed, arxiv, openalex, inspire, patentsview, lens, epo) with per-source description, required credential fields, whether credentials are configured in this environment, and rate-limit hints. Read-only; call first to decide which sources to pass to `search`. Returns: JSON array."
    )]
    fn list_sources(&self) -> Result<String, String> {
        tools::list_sources_tool()
    }

    #[tool(
        description = "Return the scoring rubric (criteria, 0.0-1.0 scale, response format). Fetch once at the start of a scoring session and cache; use with `save_assessment` or `assess_paper` for each paper. Avoids the per-paper rubric fetch that `prepare_assessment` does — if you only need the rubric (no paper context), prefer this. Returns: text."
    )]
    fn get_rubric(&self) -> Result<String, String> {
        tools::get_rubric_tool()
    }

    #[tool(
        description = "Create an annotation anchored to a passage in a paper. Root-level; use `reply_annotation` for replies. `author` is required — pass your identity string (e.g. agent slug). NOTE: author identity is trust-on-first-use until the Dolt sync / auth layer lands (Phase 5); any client may impersonate any author. Every write is logged via tracing for audit. Returns: text (the new annotation ID)."
    )]
    fn create_annotation(
        &self,
        Parameters(req): Parameters<CreateAnnotationRequest>,
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
        description = "Reply to an existing annotation. Inherits paper_id + question_id from the parent; the reply has no anchor of its own. NOTE: author identity is trust-on-first-use (see create_annotation); writes are tracing-logged. Returns: text (the new reply ID)."
    )]
    fn reply_annotation(
        &self,
        Parameters(req): Parameters<ReplyAnnotationRequest>,
    ) -> Result<String, String> {
        tools::reply_annotation_tool(&req.parent_id, &req.note, &req.author)
    }

    #[tool(
        description = "Update note / color / tags on an existing annotation. NOTE: no author check — trust-on-first-use (see create_annotation). Writes are tracing-logged. Returns: text confirmation."
    )]
    fn update_annotation(
        &self,
        Parameters(req): Parameters<UpdateAnnotationRequest>,
    ) -> Result<String, String> {
        tools::update_annotation_tool(&req.id, req.note.as_deref(), req.color.as_deref(), req.tags)
    }

    #[tool(
        description = "Soft-delete an annotation (tombstone). Threads stay intact; list_annotations hides the row. NOTE: no author check — trust-on-first-use (see create_annotation). Writes are tracing-logged. Returns: text confirmation."
    )]
    fn delete_annotation(
        &self,
        Parameters(req): Parameters<DeleteAnnotationRequest>,
    ) -> Result<String, String> {
        tools::delete_annotation_tool(&req.id)
    }

    #[tool(
        description = "List annotations for a paper. `paper_id` is required (cross-paper listing is not yet implemented). Optional `author` filter. Returns: JSON array of {id, parent_id, anchor, note, tags, author, timestamps, anchor_status}."
    )]
    fn list_annotations(
        &self,
        Parameters(req): Parameters<ListAnnotationsRequest>,
    ) -> Result<String, String> {
        tools::list_annotations_tool(Some(&req.paper_id), req.author.as_deref())
    }

    #[tool(
        description = "Mark one or more annotations as seen by `reader`. Repeat calls just update seen_at. Used so an agent can stop re-processing notes it already handled. Returns: text count."
    )]
    fn mark_seen(&self, Parameters(req): Parameters<MarkSeenRequest>) -> Result<String, String> {
        tools::mark_seen_tool(req.annotation_ids, &req.reader)
    }

    #[tool(
        description = "Mark a whole annotation thread (root + replies) as seen by `reader` in one call. Returns: text confirmation."
    )]
    fn mark_thread_seen(
        &self,
        Parameters(req): Parameters<MarkThreadSeenRequest>,
    ) -> Result<String, String> {
        tools::mark_thread_seen_tool(&req.root_id, &req.reader)
    }

    #[tool(
        description = "List annotations `reader` has not yet seen (or that were edited since last seen). Optional paper_id scopes the query. Use at session start to pick up human replies from the previous turn. NOTE: comparison is wall-clock-based (`seen_at < updated_at`), so a concurrent edit between mark_seen and list_unread can race on microsecond ordering and a non-monotonic clock rewind breaks the comparison entirely. Single-reader use is unaffected; multi-reader clients should treat unread as a hint, not a guarantee. (#100) Returns: JSON array."
    )]
    fn list_unread(
        &self,
        Parameters(req): Parameters<ListUnreadRequest>,
    ) -> Result<String, String> {
        tools::list_unread_tool(&req.reader, req.paper_id.as_deref())
    }

    #[tool(
        description = "Full-text search over stored past searches (FTS5 + Porter stemming). Sorted by relevance (lower rank = more relevant). Call before running a fresh `search` to detect redundant work. Returns: JSON array."
    )]
    fn find_similar_searches(
        &self,
        Parameters(req): Parameters<FindSimilarSearchesRequest>,
    ) -> Result<String, String> {
        tools::find_similar_searches_tool(&req.query, req.limit)
    }

    #[tool(
        description = "Summarize every paper in a search in one call: title, authors, year, abstract (truncated), DOI, identifiers. Preferred over iterating `get_paper` per result when scanning a corpus. Returns: JSON array."
    )]
    fn summarize_search(
        &self,
        Parameters(req): Parameters<SummarizeSearchRequest>,
    ) -> Result<String, String> {
        tools::summarize_search_tool(&req.search_id, req.max_papers, req.abstract_char_limit)
    }

    #[tool(description = "List recent search runs. Returns: text table.")]
    fn list_searches(
        &self,
        Parameters(req): Parameters<ListSearchesRequest>,
    ) -> Result<String, String> {
        tools::list_searches_tool(req.limit.unwrap_or(20))
    }

    #[tool(
        description = "Get papers from a search result. Returns: text listing (title, authors, year, journal, IDs, abstract preview)."
    )]
    fn get_papers(&self, Parameters(req): Parameters<SearchIdRequest>) -> Result<String, String> {
        tools::get_papers_tool(&req.search_id)
    }

    #[tool(description = "Get full details of a single paper. Returns: JSON.")]
    fn get_paper(&self, Parameters(req): Parameters<PaperIdRequest>) -> Result<String, String> {
        tools::get_paper_tool(&req.paper_id)
    }

    #[tool(
        description = "Returns: JSON {paper {id,title,abstract,full_text}, annotations[] (live only, with parent_id/root_id and full anchor incl. char_range/quote/prefix/suffix/sentence_id/source_version/status), source_version}. One call replaces get_paper + list_annotations when an agent needs to reason over offsets."
    )]
    fn get_annotated_paper(
        &self,
        Parameters(req): Parameters<PaperIdRequest>,
    ) -> Result<String, String> {
        tools::get_annotated_paper_tool(&req.paper_id)
    }

    #[tool(
        description = "Fetch the works this paper cites (forward references) via OpenAlex's `referenced_works`. Materialises each cited work as a Paper row + persists the citation edges so subsequent queries hit the local DB. Requires the source paper to have an openalex_id. Returns: JSON {source_paper_id, count, references[]}."
    )]
    async fn get_references(
        &self,
        Parameters(req): Parameters<PaperIdRequest>,
    ) -> Result<String, String> {
        tools::get_references_tool(&req.paper_id).await
    }

    #[tool(
        description = "Fetch the works that cite this paper (reverse direction) via OpenAlex's `cites:` filter. Materialises citing works + persists edges. `limit` defaults to 25, capped at 200 by the OpenAlex API. Returns: JSON {source_paper_id, count, citations[]}."
    )]
    async fn get_citations(
        &self,
        Parameters(req): Parameters<GetCitationsRequest>,
    ) -> Result<String, String> {
        tools::get_citations_tool(&req.paper_id, req.limit).await
    }

    #[tool(
        description = "Export search results in a given format. Returns: text in the requested format (JSON / CSV / BibTeX)."
    )]
    fn export_search(
        &self,
        Parameters(req): Parameters<ExportSearchRequest>,
    ) -> Result<String, String> {
        tools::export_search_tool(&req.search_id, &req.format)
    }

    #[tool(description = "Create a new research question. Returns: text confirmation with ID.")]
    fn create_question(
        &self,
        Parameters(req): Parameters<CreateQuestionRequest>,
    ) -> Result<String, String> {
        tools::create_question_tool(&req.text, &req.description)
    }

    #[tool(description = "List all research questions. Returns: text table.")]
    fn list_questions(&self) -> Result<String, String> {
        tools::list_questions_tool()
    }

    #[tool(
        description = "Add search terms linked to a research question. If `query_string` is omitted, the terms are joined by spaces. Returns: text confirmation."
    )]
    fn add_search_terms(
        &self,
        Parameters(req): Parameters<AddSearchTermsRequest>,
    ) -> Result<String, String> {
        tools::add_search_terms_tool(&req.question_id, &req.terms, req.query_string.as_deref())
    }

    #[tool(
        description = "Record a paper assessment with score and reasoning. Returns: text summary."
    )]
    fn assess_paper(
        &self,
        Parameters(req): Parameters<AssessPaperRequest>,
    ) -> Result<String, String> {
        tools::assess_paper_tool(
            &req.paper_id,
            &req.question_id,
            req.score,
            &req.reasoning,
            &req.assessor,
            req.model.as_deref(),
        )
    }

    #[tool(
        description = "Get assessments for a paper and/or question. At least one of `paper_id` or `question_id` is required (the call errors if both are omitted). Returns: text listing."
    )]
    fn get_assessments(
        &self,
        Parameters(req): Parameters<GetAssessmentsRequest>,
    ) -> Result<String, String> {
        tools::get_assessments_tool(req.paper_id.as_deref(), req.question_id.as_deref())
    }

    #[tool(
        description = "Prepare assessment rubric and paper data for LLM evaluation. Bundles `get_rubric` + the paper context for a single-call setup; if you only need the static rubric (no paper) prefer `get_rubric` to skip the paper fetch. Returns: text (rubric + paper block + instructions)."
    )]
    fn prepare_assessment(
        &self,
        Parameters(req): Parameters<PaperQuestionRequest>,
    ) -> Result<String, String> {
        tools::prepare_assessment_tool(&req.paper_id, &req.question_id)
    }

    #[tool(
        description = "Save an MCP-native assessment scored by the host LLM. Returns: text confirmation."
    )]
    fn save_assessment(
        &self,
        Parameters(req): Parameters<SaveAssessmentRequest>,
    ) -> Result<String, String> {
        tools::save_assessment_tool(&req.paper_id, &req.question_id, req.score, &req.reasoning)
    }

    #[tool(
        description = "Download a paper (PDF or HTML). Prefer passing paper_id to leverage all stored identifiers (arxiv/openalex/doi); doi is a fallback for ad-hoc lookups. Returns: text (path + access status). Emits MCP progress notifications (start + done) when the caller supplies a `progressToken` in `_meta` (#58)."
    )]
    async fn download_paper(
        &self,
        Parameters(req): Parameters<DownloadPaperRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<String, String> {
        let token = ctx.meta.get_progress_token();
        let target = req
            .paper_id
            .as_deref()
            .or(req.doi.as_deref())
            .unwrap_or("(no paper_id or doi)");
        notify(
            &ctx.peer,
            token.as_ref(),
            0.0,
            Some(1.0),
            format!("downloading {target}"),
        )
        .await;
        let result = tools::download_paper_tool(
            req.paper_id.as_deref(),
            req.doi.as_deref(),
            req.output_dir.as_deref(),
        )
        .await;
        let done_msg = match &result {
            Ok(_) => "download complete".to_string(),
            Err(e) => format!("download failed: {e}"),
        };
        notify(&ctx.peer, token.as_ref(), 1.0, Some(1.0), done_msg).await;
        result
    }

    #[tool(
        description = "Extract the text from an already-downloaded paper's PDF or HTML. Call download_paper first. Returns: text (paper title, path, extracted body, possibly truncated)."
    )]
    async fn read_paper(
        &self,
        Parameters(req): Parameters<ReadPaperRequest>,
    ) -> Result<String, String> {
        tools::read_paper_tool(&req.paper_id, req.max_chars).await
    }

    #[tool(
        description = "Prepare batch assessments for all papers in a search. Returns: text (rubric + per-paper context + instructions). Emits MCP progress notifications (start + done) when the caller supplies a `progressToken` in `_meta` (#58)."
    )]
    async fn prepare_batch_assessments(
        &self,
        Parameters(req): Parameters<SearchQuestionRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<String, String> {
        let token = ctx.meta.get_progress_token();
        notify(
            &ctx.peer,
            token.as_ref(),
            0.0,
            Some(1.0),
            format!("preparing assessments for search {}", req.search_id),
        )
        .await;
        let result = tools::prepare_batch_assessments_tool(&req.search_id, &req.question_id);
        let done_msg = match &result {
            Ok(_) => "batch prep complete".to_string(),
            Err(e) => format!("batch prep failed: {e}"),
        };
        notify(&ctx.peer, token.as_ref(), 1.0, Some(1.0), done_msg).await;
        result
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ScitadelServer {}

#[cfg(test)]
mod tests {
    /// Style gate (#98): every `#[tool(description = "...")]` must
    /// telegraph its return shape so an LLM client can parse the
    /// response without trial and error. Accepts any of:
    /// `Returns: JSON`, `Returns: text`, `Returns JSON`, `Returns text`.
    #[test]
    fn every_tool_description_states_return_shape() {
        let full = include_str!("server.rs");
        // Stop at the test module so the assertion's example string
        // (which itself contains `#[tool(...)]`) doesn't trigger the
        // gate on itself.
        let cutoff = full
            .find("#[cfg(test)]")
            .expect("test module marker present");
        let src = &full[..cutoff];
        let mut offset = 0;
        let mut tool_count = 0;
        while let Some(start) = src[offset..].find("#[tool(") {
            let abs_start = offset + start;
            let mut depth = 0;
            let mut end = abs_start;
            for (i, c) in src[abs_start..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = abs_start + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let attr = &src[abs_start..=end];
            offset = end + 1;

            if !attr.contains("description") {
                continue;
            }
            tool_count += 1;
            let ok = [
                "Returns: JSON",
                "Returns: text",
                "Returns JSON",
                "Returns text",
            ]
            .iter()
            .any(|needle| attr.contains(needle));
            assert!(
                ok,
                "tool description missing return-shape marker (one of `Returns: JSON` / `Returns: text` / `Returns JSON` / `Returns text`):\n{attr}"
            );
        }
        assert!(
            tool_count >= 25,
            "expected to scan all MCP tools (~26+); only saw {tool_count} — has the macro shape changed?"
        );
    }
}
