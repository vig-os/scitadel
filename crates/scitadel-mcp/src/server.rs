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
    pub query_string: Option<String>,
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
    #[tool(
        description = "Search scientific literature across multiple sources. Returns: JSON with search_id, query, per-source outcomes, total counts, and a `summary` text field for human readers."
    )]
    async fn search(&self, #[tool(aggr)] req: SearchRequest) -> Result<String, String> {
        tools::search_tool(req.query, req.sources, req.max_results, req.question_id).await
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
        description = "Reply to an existing annotation. Inherits paper_id + question_id from the parent; the reply has no anchor of its own. NOTE: author identity is trust-on-first-use (see create_annotation); writes are tracing-logged. Returns: text (the new reply ID)."
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

    #[tool(
        description = "Update note / color / tags on an existing annotation. NOTE: no author check — trust-on-first-use (see create_annotation). Writes are tracing-logged. Returns: text confirmation."
    )]
    fn update_annotation(
        &self,
        #[tool(aggr)] req: UpdateAnnotationRequest,
    ) -> Result<String, String> {
        tools::update_annotation_tool(&req.id, req.note.as_deref(), req.color.as_deref(), req.tags)
    }

    #[tool(
        description = "Soft-delete an annotation (tombstone). Threads stay intact; list_annotations hides the row. NOTE: no author check — trust-on-first-use (see create_annotation). Writes are tracing-logged. Returns: text confirmation."
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
        description = "List annotations for a paper. `paper_id` is required (cross-paper listing is not yet implemented). Optional `author` filter. Returns: JSON array of {id, parent_id, anchor, note, tags, author, timestamps, anchor_status}."
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
        description = "Mark one or more annotations as seen by `reader`. Repeat calls just update seen_at. Used so an agent can stop re-processing notes it already handled. Returns: text count."
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
        description = "Mark a whole annotation thread (root + replies) as seen by `reader` in one call. Returns: text confirmation."
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
        description = "List annotations `reader` has not yet seen (or that were edited since last seen). Optional paper_id scopes the query. Use at session start to pick up human replies from the previous turn. NOTE: comparison is wall-clock-based (`seen_at < updated_at`), so a concurrent edit between mark_seen and list_unread can race on microsecond ordering and a non-monotonic clock rewind breaks the comparison entirely. Single-reader use is unaffected; multi-reader clients should treat unread as a hint, not a guarantee. (#100) Returns: JSON array."
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
        description = "Full-text search over stored past searches (FTS5 + Porter stemming). Sorted by relevance (lower rank = more relevant). Call before running a fresh `search` to detect redundant work. Returns: JSON array."
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
        description = "Summarize every paper in a search in one call: title, authors, year, abstract (truncated), DOI, identifiers. Preferred over iterating `get_paper` per result when scanning a corpus. Returns: JSON array."
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

    #[tool(description = "List recent search runs. Returns: text table.")]
    fn list_searches(
        &self,
        #[tool(param)]
        #[schemars(description = "Maximum number of searches to return")]
        limit: Option<i64>,
    ) -> Result<String, String> {
        tools::list_searches_tool(limit.unwrap_or(20))
    }

    #[tool(
        description = "Get papers from a search result. Returns: text listing (title, authors, year, journal, IDs, abstract preview)."
    )]
    fn get_papers(
        &self,
        #[tool(param)]
        #[schemars(description = "Search ID")]
        search_id: String,
    ) -> Result<String, String> {
        tools::get_papers_tool(&search_id)
    }

    #[tool(description = "Get full details of a single paper. Returns: JSON.")]
    fn get_paper(
        &self,
        #[tool(param)]
        #[schemars(description = "Paper ID")]
        paper_id: String,
    ) -> Result<String, String> {
        tools::get_paper_tool(&paper_id)
    }

    #[tool(
        description = "Returns: JSON {paper {id,title,abstract,full_text}, annotations[] (live only, with parent_id/root_id and full anchor incl. char_range/quote/prefix/suffix/sentence_id/source_version/status), source_version}. One call replaces get_paper + list_annotations when an agent needs to reason over offsets."
    )]
    fn get_annotated_paper(
        &self,
        #[tool(param)]
        #[schemars(description = "Paper ID")]
        paper_id: String,
    ) -> Result<String, String> {
        tools::get_annotated_paper_tool(&paper_id)
    }

    #[tool(
        description = "Fetch the works this paper cites (forward references) via OpenAlex's `referenced_works`. Materialises each cited work as a Paper row + persists the citation edges so subsequent queries hit the local DB. Requires the source paper to have an openalex_id. Returns: JSON {source_paper_id, count, references[]}."
    )]
    async fn get_references(
        &self,
        #[tool(param)]
        #[schemars(description = "Source paper ID (must have openalex_id)")]
        paper_id: String,
    ) -> Result<String, String> {
        tools::get_references_tool(&paper_id).await
    }

    #[tool(
        description = "Fetch the works that cite this paper (reverse direction) via OpenAlex's `cites:` filter. Materialises citing works + persists edges. `limit` defaults to 25, capped at 200 by the OpenAlex API. Returns: JSON {source_paper_id, count, citations[]}."
    )]
    async fn get_citations(
        &self,
        #[tool(param)]
        #[schemars(description = "Source paper ID (must have openalex_id)")]
        paper_id: String,
        #[tool(param)]
        #[schemars(description = "Max citing works to return (default 25, max 200)")]
        limit: Option<usize>,
    ) -> Result<String, String> {
        tools::get_citations_tool(&paper_id, limit).await
    }

    #[tool(
        description = "Export search results in a given format. Returns: text in the requested format (JSON / CSV / BibTeX)."
    )]
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

    #[tool(description = "Create a new research question. Returns: text confirmation with ID.")]
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

    #[tool(description = "List all research questions. Returns: text table.")]
    fn list_questions(&self) -> Result<String, String> {
        tools::list_questions_tool()
    }

    #[tool(
        description = "Add search terms linked to a research question. If `query_string` is omitted, the terms are joined by spaces. Returns: text confirmation."
    )]
    fn add_search_terms(&self, #[tool(aggr)] req: AddSearchTermsRequest) -> Result<String, String> {
        tools::add_search_terms_tool(&req.question_id, &req.terms, req.query_string.as_deref())
    }

    #[tool(
        description = "Record a paper assessment with score and reasoning. Returns: text summary."
    )]
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

    #[tool(
        description = "Get assessments for a paper and/or question. At least one of `paper_id` or `question_id` is required (the call errors if both are omitted). Returns: text listing."
    )]
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

    #[tool(
        description = "Prepare assessment rubric and paper data for LLM evaluation. Bundles `get_rubric` + the paper context for a single-call setup; if you only need the static rubric (no paper) prefer `get_rubric` to skip the paper fetch. Returns: text (rubric + paper block + instructions)."
    )]
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

    #[tool(
        description = "Save an MCP-native assessment scored by the host LLM. Returns: text confirmation."
    )]
    fn save_assessment(&self, #[tool(aggr)] req: SaveAssessmentRequest) -> Result<String, String> {
        tools::save_assessment_tool(&req.paper_id, &req.question_id, req.score, &req.reasoning)
    }

    #[tool(
        description = "Download a paper (PDF or HTML). Prefer passing paper_id to leverage all stored identifiers (arxiv/openalex/doi); doi is a fallback for ad-hoc lookups. Returns: text (path + access status)."
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
        description = "Extract the text from an already-downloaded paper's PDF or HTML. Call download_paper first. Returns: text (paper title, path, extracted body, possibly truncated)."
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

    #[tool(
        description = "Prepare batch assessments for all papers in a search. Returns: text (rubric + per-paper context + instructions)."
    )]
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
            // Find matching close-paren by simple depth tracking.
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

            // Skip the macro-level `#[tool(tool_box)]` markers.
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
