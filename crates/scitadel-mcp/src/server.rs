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
pub struct CreatePaperNoteRequest {
    /// Paper ID this note comments on. Must already exist.
    pub paper_id: String,
    /// Note body — markdown allowed.
    pub note: String,
    /// Author identity (e.g. agent slug, user name).
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
pub struct SubscribeAnnotationsRequest {
    /// Optional paper ID to scope the subscription. When set, only
    /// events on annotations anchored to this paper are delivered.
    /// When absent, all annotation lifecycle events are delivered.
    pub paper_id: Option<String>,
    /// Subscriber identity. Recorded in the spawned task's tracing
    /// span so audit logs can attribute notifications to a specific
    /// agent. Not currently used to filter events. (#185)
    pub reader: String,
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
pub struct ToggleStarRequest {
    /// Paper ID to toggle the star on
    pub paper_id: String,
    /// Reader identity (e.g. agent slug, user name)
    pub reader: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetStarRequest {
    /// Paper ID
    pub paper_id: String,
    /// Desired starred state (true = star, false = unstar)
    pub starred: bool,
    /// Reader identity
    pub reader: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReaderRequest {
    /// Reader identity to scope the query to
    pub reader: String,
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
    /// Max characters of `full_text` to return (default 20000). When
    /// truncated, the JSON response carries `truncated: true` and
    /// `total_chars` so the agent can request more.
    pub max_chars: Option<usize>,
    /// When true (the default), the response is a JSON envelope that
    /// also includes every live annotation anchored to the paper. When
    /// false, the response is the legacy text format with no annotation
    /// information. Default-true closes the silent-miss bug where an
    /// agent calling `read_paper` would never observe a human comment.
    /// (#185)
    pub with_annotations: Option<bool>,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportBibtexRequest {
    /// Path to the `.bib` file (Zotero / Mendeley export)
    pub path: String,
    /// Merge strategy: `reject`, `db-wins`, `bib-wins`, `merge` (default).
    #[serde(default)]
    pub strategy: Option<String>,
    /// Identity attached to imported annotations (`note=` field).
    /// Required so the resulting Annotation rows have a real author.
    pub reader: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RekeyPaperRequest {
    /// Paper id to rekey.
    pub paper_id: String,
    /// Explicit citation key. If omitted, the algorithm picks one
    /// from current paper metadata (and the disambiguator excludes
    /// the paper's own existing key so a fresh suffix is possible).
    #[serde(default)]
    pub explicit_key: Option<String>,
    /// Identity recorded in the audit log.
    pub reader: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BibSnapshotRequest {
    /// Research question ID (or unique prefix) whose shortlist to snapshot
    pub question_id: String,
    /// Output path (default `paper.bib` for bibtex, `paper.json` for csl-json)
    pub output: String,
    /// Reader identity (mirrors annotation/star scoping)
    pub reader: String,
    /// Skip writing the `.scitadel-bib.lock` sidecar
    #[serde(default)]
    pub no_lock: bool,
    /// Output flavor: `bibtex` (default) or `csl-json` (canonical CSL 1.0.2).
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BibDiffRequest {
    /// First bibliography file (BibTeX or CSL-JSON; auto-detected).
    pub file_a: String,
    /// Second bibliography file. Mutually exclusive with `question_id`.
    #[serde(default)]
    pub file_b: Option<String>,
    /// Compare `file_a` against a fresh snapshot of this question's
    /// shortlist. Mutually exclusive with `file_b`.
    #[serde(default)]
    pub question_id: Option<String>,
    /// Reader scope when comparing against `question_id` (mirrors
    /// annotation/star scoping). Required when `question_id` is set.
    #[serde(default)]
    pub reader: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BibVerifyRequest {
    /// Path to the committed export to verify
    pub file: String,
    /// Override the sidecar's question_id (rarely needed)
    pub question_id: Option<String>,
    /// Reserved for future per-call overrides; today the sidecar's
    /// `format` is authoritative so verify routes to the right emitter
    /// regardless of this value.
    #[serde(default)]
    pub format: Option<String>,
}

// ---------- Server ----------

use crate::events::{AnnotationEvent, AnnotationEventKind};
use tokio::sync::broadcast;

/// Build the resource URI advertised to a `subscribe_annotations`
/// caller. Pure function so the URI-shape contract can be tested
/// without spawning the broadcast task. (#185)
#[must_use]
pub fn subscription_uri(paper_id: Option<&str>) -> String {
    paper_id.map_or_else(
        || "scitadel://annotations/all".to_string(),
        |p| format!("scitadel://annotations/{p}"),
    )
}

/// Whether a subscriber scoped to `scope_paper` should be notified
/// about an event on `event_paper`. None scope = subscribed to all
/// papers. Pure function so the routing logic is testable without
/// spawning the subscription task. (#185)
#[must_use]
pub fn event_matches_scope(scope_paper: Option<&str>, event_paper: &str) -> bool {
    scope_paper.is_none_or(|p| p == event_paper)
}

#[derive(Debug, Clone)]
pub struct ScitadelServer {
    tool_router: ToolRouter<Self>,
    /// Broadcast channel for annotation lifecycle events (#185 P0).
    /// Every write tool emits one event; the `subscribe_annotations`
    /// tool (PR3 C2) hands a `Receiver` to a server-side task that
    /// translates events into MCP `notifications/resources/updated`
    /// for its peer. Cloning the server clones the `Sender`, which
    /// is the intended sharing model.
    event_tx: broadcast::Sender<AnnotationEvent>,
}

impl ScitadelServer {
    #[must_use]
    pub fn new() -> Self {
        // Discard the receiver from the initial pair so `Sender::send`
        // returns SendError (fail-silent) until a real subscriber
        // attaches via `event_tx.subscribe()`. The Sender stays
        // usable for future subscribe() calls.
        let (event_tx, _) = crate::events::channel();
        Self {
            tool_router: Self::tool_router(),
            event_tx,
        }
    }

    /// Hand out a fresh receiver for the annotation event stream.
    /// Used by the `subscribe_annotations` tool (PR3 C2). Subscribers
    /// that lag past the channel capacity see `RecvError::Lagged(n)`
    /// on `recv` — they should re-fetch via `list_annotations` to
    /// resync.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<AnnotationEvent> {
        self.event_tx.subscribe()
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
        description = "Create an annotation anchored to a passage in a paper. Root-level; use `reply_annotation` for replies. `author` is required — pass your identity string (e.g. agent slug). NOTE: author identity is trust-on-first-use until the Dolt sync / auth layer lands (Phase 5); any client may impersonate any author. Every write is logged via tracing for audit and emits an MCP-spec `notifications/resources/updated` to active `subscribe_annotations` clients (#185). Returns: text (the new annotation ID)."
    )]
    fn create_annotation(
        &self,
        Parameters(req): Parameters<CreateAnnotationRequest>,
    ) -> Result<String, String> {
        let id = tools::create_annotation_tool(
            &req.paper_id,
            &req.quote,
            &req.note,
            &req.author,
            req.prefix.as_deref(),
            req.suffix.as_deref(),
            req.question_id.as_deref(),
            req.color.as_deref(),
            req.tags,
        )?;
        crate::events::emit(
            &self.event_tx,
            AnnotationEvent {
                paper_id: req.paper_id.clone(),
                annotation_id: id.clone(),
                kind: AnnotationEventKind::Created,
                reader: None,
            },
        );
        Ok(id)
    }

    #[tool(
        description = "Reply to an existing annotation. Inherits paper_id + question_id from the parent; the reply has no anchor of its own. NOTE: author identity is trust-on-first-use (see create_annotation); writes are tracing-logged. Emits an MCP-spec `notifications/resources/updated` to active `subscribe_annotations` clients (#185). Returns: text (the new reply ID)."
    )]
    fn reply_annotation(
        &self,
        Parameters(req): Parameters<ReplyAnnotationRequest>,
    ) -> Result<String, String> {
        // Look up the parent's paper_id BEFORE the reply so the
        // emitted event scope is correct even if the parent was
        // somehow deleted between this and the next read.
        let parent_paper = tools::lookup_annotation_paper_id(&req.parent_id)?;
        let id = tools::reply_annotation_tool(&req.parent_id, &req.note, &req.author)?;
        if let Some(paper_id) = parent_paper {
            crate::events::emit(
                &self.event_tx,
                AnnotationEvent {
                    paper_id,
                    annotation_id: id.clone(),
                    kind: AnnotationEventKind::Replied,
                    reader: None,
                },
            );
        }
        Ok(id)
    }

    #[tool(
        description = "Comment on a paper as a whole — quote-less, anchor-less commentary. Use this when the observation isn't tied to a specific passage (overall reaction, methodology critique, taxonomy tag). The TUI renders these in a dedicated 'paper-level notes' section above the threaded annotations. NOTE: author identity is trust-on-first-use (see create_annotation); writes are tracing-logged. Emits an MCP-spec `notifications/resources/updated` to active `subscribe_annotations` clients (#185). Returns: text (the new annotation ID)."
    )]
    fn create_paper_note(
        &self,
        Parameters(req): Parameters<CreatePaperNoteRequest>,
    ) -> Result<String, String> {
        let id = tools::create_paper_note_tool(&req.paper_id, &req.note, &req.author)?;
        crate::events::emit(
            &self.event_tx,
            AnnotationEvent {
                paper_id: req.paper_id.clone(),
                annotation_id: id.clone(),
                kind: AnnotationEventKind::Created,
                reader: None,
            },
        );
        Ok(id)
    }

    #[tool(
        description = "Update note / color / tags on an existing annotation. NOTE: no author check — trust-on-first-use (see create_annotation). Writes are tracing-logged. Emits an MCP-spec `notifications/resources/updated` to active `subscribe_annotations` clients (#185). Returns: text confirmation."
    )]
    fn update_annotation(
        &self,
        Parameters(req): Parameters<UpdateAnnotationRequest>,
    ) -> Result<String, String> {
        let paper_id = tools::lookup_annotation_paper_id(&req.id)?;
        let result = tools::update_annotation_tool(
            &req.id,
            req.note.as_deref(),
            req.color.as_deref(),
            req.tags,
        )?;
        if let Some(paper_id) = paper_id {
            crate::events::emit(
                &self.event_tx,
                AnnotationEvent {
                    paper_id,
                    annotation_id: req.id.clone(),
                    kind: AnnotationEventKind::Updated,
                    reader: None,
                },
            );
        }
        Ok(result)
    }

    #[tool(
        description = "Soft-delete an annotation (tombstone). Threads stay intact; list_annotations hides the row. NOTE: no author check — trust-on-first-use (see create_annotation). Writes are tracing-logged. Emits an MCP-spec `notifications/resources/updated` to active `subscribe_annotations` clients (#185). Returns: text confirmation."
    )]
    fn delete_annotation(
        &self,
        Parameters(req): Parameters<DeleteAnnotationRequest>,
    ) -> Result<String, String> {
        // Look up paper_id BEFORE the soft-delete — afterwards
        // `repo.get` filters out the tombstoned row and we'd lose the
        // scope information for the event.
        let paper_id = tools::lookup_annotation_paper_id(&req.id)?;
        let result = tools::delete_annotation_tool(&req.id)?;
        if let Some(paper_id) = paper_id {
            crate::events::emit(
                &self.event_tx,
                AnnotationEvent {
                    paper_id,
                    annotation_id: req.id.clone(),
                    kind: AnnotationEventKind::Deleted,
                    reader: None,
                },
            );
        }
        Ok(result)
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
        description = "Mark one or more annotations as seen by `reader`. Repeat calls just update seen_at. Used so an agent can stop re-processing notes it already handled. Emits one `notifications/resources/updated` per annotation_id to active `subscribe_annotations` clients (#185). Returns: text count."
    )]
    fn mark_seen(&self, Parameters(req): Parameters<MarkSeenRequest>) -> Result<String, String> {
        // Resolve paper_id per id BEFORE the write so the event
        // scope is correct (the row exists pre-write; mark_seen is
        // not destructive but we keep the pattern uniform with the
        // other mutating tools).
        let scopes: Vec<(String, Option<String>)> = req
            .annotation_ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    tools::lookup_annotation_paper_id(id).ok().flatten(),
                )
            })
            .collect();
        let result = tools::mark_seen_tool(req.annotation_ids, &req.reader)?;
        for (annotation_id, paper_id) in scopes {
            if let Some(paper_id) = paper_id {
                crate::events::emit(
                    &self.event_tx,
                    AnnotationEvent {
                        paper_id,
                        annotation_id,
                        kind: AnnotationEventKind::MarkedSeen,
                        reader: Some(req.reader.clone()),
                    },
                );
            }
        }
        Ok(result)
    }

    #[tool(
        description = "Mark a whole annotation thread (root + replies) as seen by `reader` in one call. Emits one `notifications/resources/updated` (kind=marked_thread_seen, annotation_id=root_id) to active `subscribe_annotations` clients (#185). Returns: text confirmation."
    )]
    fn mark_thread_seen(
        &self,
        Parameters(req): Parameters<MarkThreadSeenRequest>,
    ) -> Result<String, String> {
        let paper_id = tools::lookup_annotation_paper_id(&req.root_id)?;
        let result = tools::mark_thread_seen_tool(&req.root_id, &req.reader)?;
        if let Some(paper_id) = paper_id {
            crate::events::emit(
                &self.event_tx,
                AnnotationEvent {
                    paper_id,
                    annotation_id: req.root_id.clone(),
                    kind: AnnotationEventKind::MarkedThreadSeen,
                    reader: Some(req.reader.clone()),
                },
            );
        }
        Ok(result)
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
        description = "Subscribe to annotation lifecycle events on this paper (or all papers if `paper_id` is omitted). Spawns a server-side task that translates each create/reply/update/delete/mark_seen event into an MCP-spec `notifications/resources/updated` for the calling peer. The notification carries only the resource URI — call `list_annotations` (or `list_unread`) to inspect what changed. Lagged subscribers see one suppressed notification then resume; re-fetch via `list_annotations` to recover. Returns: text (the resource URI, e.g. `scitadel://annotations/{paper_id_or_all}`). (#185)"
    )]
    async fn subscribe_annotations(
        &self,
        Parameters(req): Parameters<SubscribeAnnotationsRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<String, String> {
        // Reject `Some("")` explicitly: an empty paper_id would
        // produce `scitadel://annotations/` and a scope filter that
        // never matches a real event, leaving the caller with a
        // valid-looking URI that delivers nothing. Treat as caller
        // error rather than degrading silently. (#185)
        if req.paper_id.as_deref() == Some("") {
            return Err("paper_id, if provided, must not be empty".into());
        }
        let uri = subscription_uri(req.paper_id.as_deref());
        let mut rx = self.event_tx.subscribe();
        let peer = ctx.peer.clone();
        let scope_paper = req.paper_id.clone();
        let reader = req.reader.clone();
        let uri_for_task = uri.clone();
        // Spawn one task per subscribe call. The task lives until
        // either the broadcast Sender drops (server shutdown) or the
        // peer's notify call fails (client disconnect). No explicit
        // unsubscribe RPC is needed; closing the connection is the
        // unsubscribe signal.
        tokio::spawn(async move {
            tracing::debug!(reader, uri = uri_for_task, "annotation subscriber active");
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if !event_matches_scope(scope_paper.as_deref(), &event.paper_id) {
                            continue;
                        }
                        let result = peer
                            .notify_resource_updated(
                                rmcp::model::ResourceUpdatedNotificationParam {
                                    uri: uri_for_task.clone(),
                                },
                            )
                            .await;
                        if let Err(e) = result {
                            // Peer disconnected (or transport error).
                            // Drop the subscription — the receiver
                            // would otherwise leak the task forever.
                            tracing::debug!(reader, uri = uri_for_task, error = %e, "annotation peer notify failed; ending subscription");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // We dropped `n` events. The notification
                        // payload only carries the URI anyway, so
                        // emit one update — the agent should re-fetch
                        // via list_annotations to catch up.
                        tracing::warn!(
                            reader,
                            uri = uri_for_task,
                            dropped = n,
                            "annotation subscriber lagged; emitting one resync update"
                        );
                        let _ = peer
                            .notify_resource_updated(
                                rmcp::model::ResourceUpdatedNotificationParam {
                                    uri: uri_for_task.clone(),
                                },
                            )
                            .await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Sender dropped — server is shutting down.
                        break;
                    }
                }
            }
            tracing::debug!(reader, uri = uri_for_task, "annotation subscriber ended");
        });
        Ok(uri)
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
        description = "Returns: JSON {paper {id,title,abstract,full_text}, annotations[] (live only, with parent_id/root_id and full anchor incl. char_range/quote/prefix/suffix/sentence_id/source_version/status), source_version}. One call replaces get_paper + list_annotations when an agent needs to reason over offsets. NOTE: `read_paper` (with default `with_annotations: true`) returns the same envelope plus extractor metadata and PDF/HTML extraction; prefer it when you need the full text. (#185)"
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
        description = "Extract the text from an already-downloaded paper's PDF or HTML. Call download_paper first. By default (`with_annotations: true`, #185) Returns: JSON `{paper {id,title,abstract,full_text}, annotations[] (live, with parent_id/root_id and full anchor), source_version, extractor, path, truncated, total_chars}` so an agent never silently misses comments — pass `with_annotations: false` for the legacy text shape (paper title + path + extractor + body)."
    )]
    async fn read_paper(
        &self,
        Parameters(req): Parameters<ReadPaperRequest>,
    ) -> Result<String, String> {
        tools::read_paper_tool(&req.paper_id, req.max_chars, req.with_annotations).await
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

    #[tool(
        description = "Read the open scitadel TUI's current selection. Use to answer 'what is the user looking at right now?' so you can score the open paper / draft from current context without the user pasting IDs. Returns JSON `{tab, paper_id?, search_id?, question_id?, annotation_id?, updated_at, stale}`. `stale: true` means the singleton row is >60s old — TUI may have been closed. (#122)"
    )]
    fn get_current_selection(&self) -> Result<String, String> {
        tools::get_current_selection_tool()
    }

    #[tool(
        description = "Toggle the starred flag for a paper under `reader`. Creates the per-reader state row if missing. NOTE: author/reader identity is trust-on-first-use — real auth ships with the Phase-5 Dolt sync layer. Returns: JSON `{paper_id, starred}` with the new value."
    )]
    fn toggle_star(
        &self,
        Parameters(req): Parameters<ToggleStarRequest>,
    ) -> Result<String, String> {
        tools::toggle_star_tool(&req.paper_id, &req.reader)
    }

    #[tool(
        description = "Set the starred flag for a paper under `reader` to an explicit value. Idempotent — call this when you want \"ensure starred\" semantics rather than a toggle. NOTE: trust-on-first-use identity (#100). Returns: JSON `{paper_id, starred}`."
    )]
    fn set_star(&self, Parameters(req): Parameters<SetStarRequest>) -> Result<String, String> {
        tools::set_star_tool(&req.paper_id, req.starred, &req.reader)
    }

    #[tool(
        description = "List all paper IDs `reader` has starred. Returns: JSON array of paper ID strings (no metadata — call `get_paper` for each if you need title/authors)."
    )]
    fn list_starred(&self, Parameters(req): Parameters<ReaderRequest>) -> Result<String, String> {
        tools::list_starred_tool(&req.reader)
    }

    #[tool(
        description = "Import a `.bib` file (BibTeX or BibLaTeX). Matches entries against existing papers via DOI → arXiv → PubMed → OpenAlex → scitadel citekey → previously-imported alias → title+year. Imported citekeys are recorded as aliases so re-importing the same file is a no-op (round-trip safe). Zotero `note=` becomes an unanchored Annotation under `reader`; `keywords=` ride along as that annotation's tags; `file=` paths are dropped (foreign-machine paths are meaningless) and surfaced in the report. Strategies (default `merge`): `reject` skips on match, `db-wins` keeps DB untouched, `bib-wins` overwrites DB fields with bib values where present, `merge` keeps DB on owned fields and folds in non-owned side effects. Returns: JSON `{rows, failed, totals}` per-paper summary."
    )]
    fn import_bibtex(
        &self,
        Parameters(req): Parameters<ImportBibtexRequest>,
    ) -> Result<String, String> {
        tools::import_bibtex_tool(&req.path, req.strategy.as_deref(), &req.reader)
    }

    #[tool(
        description = "Reassign a paper's citation key (escape hatch for the #132 stable-key contract). Without `explicit_key`, re-runs the algorithm against the paper's current metadata; with it, sets that key directly. The pre-rekey key is preserved as an alias on `paper_aliases` so manuscripts still citing by the old key continue resolving via the import-match cascade's alias step. Fails loudly on collision with another paper's existing key. Audit-logged via `tracing::info!` with op=bib_rekey. Returns: JSON `{paper_id, old_key, new_key, changed}`."
    )]
    fn rekey_paper(
        &self,
        Parameters(req): Parameters<RekeyPaperRequest>,
    ) -> Result<String, String> {
        tools::rekey_paper_tool(&req.paper_id, req.explicit_key.as_deref(), &req.reader)
    }

    #[tool(
        description = "Snapshot a question's citation shortlist to a deterministic export file plus a `.scitadel-bib.lock` sidecar (drift/stale anchor; #178, #135). Default format is BibTeX (`.bib`); pass `format=\"csl-json\"` to emit canonical CSL-JSON 1.0.2 (`.json`). Same content twice ⇒ byte-identical output (sidecar `generated_at` excepted). Pass `no_lock=true` to skip the sidecar (CI one-offs). Returns: JSON `{output, papers, sidecar?, shortlist_hash?, content_hash?, format?}`."
    )]
    fn bib_snapshot(
        &self,
        Parameters(req): Parameters<BibSnapshotRequest>,
    ) -> Result<String, String> {
        let fmt = req.format.as_deref().unwrap_or("bibtex");
        tools::bib_snapshot_tool(&req.question_id, &req.output, &req.reader, req.no_lock, fmt)
    }

    #[tool(
        description = "Verify a `.bib` against its sidecar lockfile (#178). Computes shortlist + content hashes, compares with the lockfile, recognizes algo/version drift. Returns: JSON `{status, exit_code, ...}` where status is `ok` (0), `drift` (1: shortlist or content changed), or `stale` (2: lockfile absent or binary moved); shells should map exit_code to their process status."
    )]
    fn bib_verify(&self, Parameters(req): Parameters<BibVerifyRequest>) -> Result<String, String> {
        tools::bib_verify_tool(&req.file, req.question_id.as_deref())
    }

    #[tool(
        description = "Structural diff between two bibliographies — added/removed/changed entries with per-field changes — instead of the line-level diff `bib verify` prints. Auto-detects BibTeX vs CSL-JSON by content sniff (extension is cosmetic). Identity rule: citekey → DOI → arxiv_id → title+year (first match wins). Pass `file_b` for file-vs-file or `question_id` for file-vs-fresh-snapshot. Returns: JSON `{added: [...], removed: [...], changed: [{citekey, before_citekey?, field_changes: [...]}]}` — empty arrays mean no drift."
    )]
    fn bib_diff(&self, Parameters(req): Parameters<BibDiffRequest>) -> Result<String, String> {
        tools::bib_diff_tool(
            &req.file_a,
            req.file_b.as_deref(),
            req.question_id.as_deref(),
            req.reader.as_deref(),
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ScitadelServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "scitadel".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use rmcp::ServerHandler;

    /// Scope filter: a subscriber with `paper_id=None` sees every
    /// event; a paper-scoped subscriber sees only events on that
    /// paper. (#185)
    #[test]
    fn event_matches_scope_filter() {
        assert!(super::event_matches_scope(None, "p-a"));
        assert!(super::event_matches_scope(None, "p-b"));
        assert!(super::event_matches_scope(Some("p-a"), "p-a"));
        assert!(!super::event_matches_scope(Some("p-a"), "p-b"));
        // Empty-string scope only matches empty-string event paper_id
        // — defensive (the public call rejects empty paper_id at the
        // tool boundary or just produces a never-matching URI).
        assert!(super::event_matches_scope(Some(""), ""));
        assert!(!super::event_matches_scope(Some(""), "p-a"));
    }

    /// URI shape contract: scope-all when `paper_id` is None,
    /// `scitadel://annotations/{paper_id}` otherwise. (#185)
    #[test]
    fn subscription_uri_shape() {
        assert_eq!(super::subscription_uri(None), "scitadel://annotations/all");
        assert_eq!(
            super::subscription_uri(Some("p-attn")),
            "scitadel://annotations/p-attn"
        );
        // Empty paper_id is not a valid scope but the URI builder
        // should not panic — caller-side validation is the
        // appropriate place to reject this if it ever becomes a
        // concern. Pin the no-panic behaviour.
        assert_eq!(super::subscription_uri(Some("")), "scitadel://annotations/");
    }

    /// All env-var-mutating tests in this module share a single
    /// mutex so SCITADEL_DB doesn't get clobbered by parallel runs.
    /// The lock is held for the duration of each test; the Database
    /// is opened from the env var inside the same lock, so the
    /// effective DB path is stable per test even though
    /// `cargo test` launches them concurrently.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// End-to-end: a real `create_annotation` call goes through the
    /// tool surface and emits a `Created` event on the broadcast
    /// channel. Insurance against a future refactor that re-routes a
    /// write tool through a path that skips `events::emit` — without
    /// this test, the wiring at every emit site is verified only by
    /// reading the diff. Drives one tool path end-to-end; the other
    /// five are mechanically identical so this guards them all.
    /// (#185 PR3 review)
    #[tokio::test]
    async fn create_annotation_through_server_emits_created_event() {
        // Hold the env-var lock for the sync setup (env-var write +
        // db open + repo save + server.create_*). All those calls
        // are sync; we drop the guard *before* the only .await so
        // the lock isn't held across an await point. Each test
        // releases the lock as soon as the broadcast Sender has
        // already been populated for *this* server instance, so a
        // parallel test taking the lock next can't disturb us.
        let (id, mut rx) = {
            let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let db_path = tmp.path().join("test.db");
            unsafe {
                std::env::set_var("SCITADEL_DB", &db_path);
            }

            let db = scitadel_db::sqlite::Database::open(&db_path).unwrap();
            db.migrate().unwrap();
            let mut p = scitadel_core::models::Paper::new("e2e");
            p.id = scitadel_core::models::PaperId::from("p-e2e");
            let (paper_repo, _, _, _, _) = db.repositories();
            scitadel_core::ports::PaperRepository::save(&paper_repo, &p).unwrap();

            let server = super::ScitadelServer::new();
            let rx = server.subscribe_events();
            let req = super::CreateAnnotationRequest {
                paper_id: "p-e2e".into(),
                quote: "Q".into(),
                note: "N".into(),
                author: "claude".into(),
                prefix: None,
                suffix: None,
                question_id: None,
                color: None,
                tags: None,
            };
            let id = server
                .create_annotation(rmcp::handler::server::wrapper::Parameters(req))
                .expect("create_annotation succeeds");
            (id, rx)
        };
        // Lock dropped — safe to await on a broadcast channel that's
        // already populated for *our* `rx`.
        let event = rx.recv().await.expect("event arrives");
        assert_eq!(event.paper_id, "p-e2e");
        assert_eq!(event.kind, super::AnnotationEventKind::Created);
        assert_eq!(event.annotation_id, id);
        assert!(event.reader.is_none(), "create events have no reader");
    }

    /// End-to-end paper-note write: the new MCP tool persists with
    /// the `paper-note:<paper_id>` sentinel and emits a `Created`
    /// event on the broadcast channel. (#185 PR4)
    #[tokio::test]
    async fn create_paper_note_through_server_persists_and_emits() {
        // Same lock-then-drop-before-await pattern as the
        // create_annotation e2e test above — sync setup + write
        // happen under the lock, the .await is on an already-
        // populated broadcast channel.
        let (id, mut rx, db) = {
            let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let db_path = tmp.path().join("test.db");
            unsafe {
                std::env::set_var("SCITADEL_DB", &db_path);
            }
            let db = scitadel_db::sqlite::Database::open(&db_path).unwrap();
            db.migrate().unwrap();
            let mut p = scitadel_core::models::Paper::new("paper-note-target");
            p.id = scitadel_core::models::PaperId::from("p-pn");
            let (paper_repo, _, _, _, _) = db.repositories();
            scitadel_core::ports::PaperRepository::save(&paper_repo, &p).unwrap();

            let server = super::ScitadelServer::new();
            let rx = server.subscribe_events();
            let req = super::CreatePaperNoteRequest {
                paper_id: "p-pn".into(),
                note: "overall: methodology weak".into(),
                author: "claude".into(),
            };
            let id = server
                .create_paper_note(rmcp::handler::server::wrapper::Parameters(req))
                .expect("create_paper_note succeeds");
            (id, rx, db)
        };

        let event = rx.recv().await.expect("event arrives");
        assert_eq!(event.paper_id, "p-pn");
        assert_eq!(event.annotation_id, id);
        assert_eq!(event.kind, super::AnnotationEventKind::Created);

        let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);
        let stored = repo.get(&id).unwrap().expect("annotation persisted");
        assert!(stored.anchor.is_paper_note());
        assert!(stored.anchor.quote.is_none());
    }

    /// `create_paper_note` rejects an empty note and a missing paper
    /// at the boundary so a typo doesn't create a dangling note with
    /// no UI surface to find it again.
    #[tokio::test]
    async fn create_paper_note_rejects_empty_note_and_missing_paper() {
        // The std::sync::Mutex guard would normally need to drop
        // before .await, but the env-var protection here only needs
        // to hold for the duration of THIS test's setup +
        // server.create_*() call (which is sync). The single .await
        // afterwards (rx.recv) waits on a broadcast already populated
        // synchronously, so holding the guard is benign in practice.
        #[allow(clippy::await_holding_lock)]
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        unsafe {
            std::env::set_var("SCITADEL_DB", &db_path);
        }
        let db = scitadel_db::sqlite::Database::open(&db_path).unwrap();
        db.migrate().unwrap();

        let server = super::ScitadelServer::new();

        let err = server
            .create_paper_note(rmcp::handler::server::wrapper::Parameters(
                super::CreatePaperNoteRequest {
                    paper_id: "p-missing".into(),
                    note: "any".into(),
                    author: "claude".into(),
                },
            ))
            .expect_err("missing paper should reject");
        assert!(err.contains("not found"));

        // Seed a paper so the second case (empty note) is the only
        // failure mode under test.
        let mut p = scitadel_core::models::Paper::new("p");
        p.id = scitadel_core::models::PaperId::from("p-pn-empty");
        let (paper_repo, _, _, _, _) = db.repositories();
        scitadel_core::ports::PaperRepository::save(&paper_repo, &p).unwrap();

        let err = server
            .create_paper_note(rmcp::handler::server::wrapper::Parameters(
                super::CreatePaperNoteRequest {
                    paper_id: "p-pn-empty".into(),
                    note: "   ".into(),
                    author: "claude".into(),
                },
            ))
            .expect_err("whitespace-only note should reject");
        assert!(err.contains("note"));
    }

    /// `subscribe_events` must hand out a receiver that observes
    /// every event emitted on the server's broadcast channel —
    /// otherwise a `subscribe_annotations` client (#185 PR3 C2) would
    /// silently miss writes. We exercise the wiring by emitting an
    /// event through the same Sender the server holds and asserting
    /// the receiver sees it.
    #[tokio::test]
    async fn subscribe_events_sees_emits_through_server_sender() {
        use crate::events::{AnnotationEvent, AnnotationEventKind};
        let server = super::ScitadelServer::new();
        let mut rx = server.subscribe_events();
        crate::events::emit(
            &server.event_tx,
            AnnotationEvent {
                paper_id: "p-1".into(),
                annotation_id: "ann-1".into(),
                kind: AnnotationEventKind::Created,
                reader: None,
            },
        );
        let got = rx.recv().await.expect("event arrives");
        assert_eq!(got.paper_id, "p-1");
        assert_eq!(got.annotation_id, "ann-1");
        assert_eq!(got.kind, AnnotationEventKind::Created);
    }

    /// An MCP server that does not declare the `tools` capability in its
    /// `initialize` response will never have `tools/list` called by the
    /// client — the server appears to expose zero tools even though the
    /// router is fully populated. `#[tool_handler]` does NOT emit a
    /// `get_info()` override, so `ServerHandler`'s default kicks in and
    /// returns empty capabilities. Guard against the regression.
    #[test]
    fn get_info_declares_tools_capability_and_server_name() {
        let info = super::ScitadelServer::new().get_info();
        assert!(
            info.capabilities.tools.is_some(),
            "get_info() must advertise `tools` capability — otherwise clients skip tools/list. capabilities={:?}",
            info.capabilities
        );
        assert_eq!(
            info.server_info.name, "scitadel",
            "server_info.name should be 'scitadel', not rmcp's default"
        );
    }

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
