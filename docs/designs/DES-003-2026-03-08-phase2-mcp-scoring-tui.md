# DES-003: Phase 2 — MCP Server, LLM Scoring, Snowball, TUI, Chat Engine

<!-- markdownlint-disable MD022 MD032 MD060 -->

| Field | Value |
|---|---|
| **Status** | accepted |
| **Created** | 2026-03-08 |
| **Updated** | 2026-03-08 |
| **Depends on** | DES-001 (Phase 1 architecture) |

## Overview

Phase 2 extends Scitadel from a federated search engine into an LLM-assisted research
workflow tool. This document covers five new capabilities built on top of the Phase 1
architecture (DES-001):

1. **MCP Server** — Exposes Scitadel tools to LLM agents via Model Context Protocol
2. **LLM Relevance Scoring** — Automated paper relevance assessment using Claude
3. **Snowball (Citation Chaining)** — Forward/backward citation graph expansion
4. **TUI Dashboard** — Interactive terminal interface using Textual
5. **Chat Engine** — Agentic loop with tool dispatch for conversational research

## Architecture Decisions

### AD-1: MCP as primary LLM integration surface

**Decision:** Expose Scitadel functionality as MCP tools rather than building a custom agent framework.

**Rationale:**
- MCP is the emerging standard for LLM tool integration
- Works with Claude Desktop, Cursor, Claude CLI without custom code
- Same tool definitions serve both MCP and internal chat engine
- Lower maintenance than a bespoke agent protocol

**Implementation:** `src/scitadel/mcp_server.py` using `mcp.server.fastmcp.FastMCP`.
All database operations use context managers (`with _get_db() as db:`) to prevent
resource leaks across the 15+ tool endpoints.

### AD-2: Synchronous scoring with sync/async parity

**Decision:** Provide both `score_paper()` (sync) and `score_paper_async()` (async)
with shared prompt-building and response-parsing logic.

**Rationale:**
- CLI scoring uses synchronous Anthropic client (simpler, no event loop)
- TUI chat engine uses async client (non-blocking UI)
- Shared `_build_user_prompt()` and `_build_assessment()` prevent divergence

**Implementation:** `src/scitadel/services/scoring.py`. Both functions delegate to
shared helpers, keeping the sync/async wrappers thin.

### AD-3: Protocol-based snowball extensibility

**Decision:** Define `CitationFetcher`, `PaperResolver`, and `Scorer` as Python
Protocols rather than abstract base classes.

**Rationale:**
- Allows duck-typed implementations without inheritance
- Test mocks are simpler (just implement the method)
- Concrete implementations can vary by context (CLI vs TUI vs MCP)

**Implementation:** `src/scitadel/services/snowball.py`. The snowball loop accepts
any object satisfying the protocol. Scorer is optional (None = no relevance gating).

### AD-4: Shared prefix resolution

**Decision:** Extract the duplicated ID prefix-matching pattern into
`src/scitadel/services/resolve.py`.

**Rationale:**
- Prefix matching was independently implemented in mcp_server.py, cli.py, and tui/data.py
- Single implementation ensures consistent behavior (exact match priority, single-match-only)
- Reduces surface area for bugs in ID resolution

### AD-5: Bounded chat history

**Decision:** Chat engine trims message history to a sliding window (default 40 messages)
that never splits tool_use/tool_result pairs.

**Rationale:**
- Unbounded history would eventually exceed Claude's context window
- Naively truncating could split a tool_use from its tool_result, causing API errors
- 40 messages provides enough context for multi-step research workflows

## Component Details

### MCP Server (`src/scitadel/mcp_server.py`)

Tools exposed:
- `search` — Federated literature search
- `list_searches` — Search history
- `get_papers` / `get_paper` — Paper retrieval
- `export_search` — BibTeX/JSON/CSV export
- `create_question` / `list_questions` / `add_search_terms` — Research question management
- `assess_paper` / `get_assessments` — Relevance scoring
- `snowball_search` — Citation chaining
- `save_paper_text` / `get_paper_text` / `fetch_paper_text` — Full text management

Entry point: `scitadel-mcp` (configured in `pyproject.toml`).

### LLM Scoring Service (`src/scitadel/services/scoring.py`)

- Structured prompt with scoring rubric (0.0–1.0 scale)
- JSON response parsing with markdown code block handling
- Batch scoring with progress callbacks and error-resilient continuation
- Failed scores recorded as 0.0 with error provenance

### Snowball Service (`src/scitadel/services/snowball.py`)

- Configurable: direction (references/cited_by/both), max_depth (hard cap: 3), threshold, max_papers_per_level
- Deduplication via `seen_ids` set
- Relevance-gated expansion: only papers above threshold enter next frontier
- Citation edges recorded with direction, depth, and discovery provenance

Citation data source: OpenAlex API via `src/scitadel/adapters/openalex/citations.py`.

### TUI Dashboard (`src/scitadel/tui/`)

Built with Textual. Screens:
- Paper browser — paginated paper list with search
- Search browser — search history and results
- Paper detail — full paper metadata
- Citation tree — citation graph visualization
- Research assistant — chat interface powered by the chat engine
- Questions — research question management
- Live search — real-time search with results

Data layer: `DataStore` (`src/scitadel/tui/data.py`) wraps repositories with
lifecycle management and prefix resolution.

### Chat Engine (`src/scitadel/services/chat_engine.py`)

- Agentic loop: sends user message → processes response → dispatches tool calls → loops until done
- `ToolDispatcher` maps tool names to handlers executing against the shared `DataStore`
- Yields streaming events (`TextDelta`, `ToolCallStart`, `ToolCallResult`, `TurnComplete`)
- Message history bounded with sliding window (WU-04)

## Data Model Extensions

Phase 2 added the following to the Phase 1 schema:

### Migration 002: Citations
- `citations` table — (source_paper_id, target_paper_id, direction, discovered_by, depth, snowball_run_id)
- `snowball_runs` table — (id, search_id, question_id, direction, max_depth, threshold, total_discovered, total_new_papers)

### Migration 003: Full Text
- `papers.full_text` column — stored full text content
- `papers.summary` column — human or LLM-generated summary

## Known Gaps

1. **No streaming in MCP tools** — MCP tool responses are returned as complete strings, not streamed. Long-running operations (snowball, batch scoring) block until complete.

2. **Scoring model hardcoded** — Default model is `claude-sonnet-4-6`. No fallback to cheaper models for large batches.

3. **No pagination in MCP prefix resolution** — `list_all(limit=1000)` is used for paper prefix matching. Will not scale beyond ~1000 papers.

4. **Snowball has no persistence checkpointing** — If a deep snowball run fails midway, discovered papers are saved but the run summary may be lost.

5. **Chat engine has no conversation persistence** — Message history exists only in memory. Restarting the TUI loses all conversation context.

6. **No rate limiting for scoring API calls** — Batch scoring calls Claude API sequentially without rate limiting. Large batches may hit API rate limits.

## Testing

| Area | Tests | File |
|---|---|---|
| Snowball service | Basic snowball, threshold gating, depth limiting, dedup, both directions, circular citations, max_papers_per_level, scorer exceptions | `tests/test_snowball.py` |
| Citation repository | Save, get, upsert depth, exists, cited_by, snowball run persistence | `tests/test_snowball.py` |
| Scoring response parsing | Covered by `tests/test_scoring.py` |
| MCP tools | `tests/test_mcp.py` |
| Chat engine message trimming | `tests/test_services.py` |
| Prefix resolution | `tests/test_resolve.py` |

## Relationship to Rust Rewrite (DES-002)

DES-002 defines the Rust rewrite plan. Phase 2 features implemented in Python serve as
the validated prototype. The Rust rewrite will re-implement these features with:
- `rmcp` for MCP server
- Native async with Tokio
- WASM Typst plugin integration
- Tauri desktop application

The Python implementation remains the reference for behavior and API contracts.
