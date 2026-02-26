# RFC-001: Scitadel Problem Space — Programmable, Reproducible Scientific Literature Retrieval

| Field       | Value                          |
|-------------|--------------------------------|
| **Status**  | proposed                       |
| **Authors** | Lars Gerchow                   |
| **Created** | 2026-02-26                     |
| **Updated** | 2026-02-26                     |

## Problem Statement

### The core problem

Scientific literature retrieval is fragmented, manual, and non-reproducible.
Researchers working across domains — radiopharma, PET imaging, detector
physics — must search multiple databases (PubMed, arXiv, INSPIRE-HEP,
OpenAlex) independently, each with different query syntax, metadata schemas,
and export formats. There is no unified, scriptable interface.

### The current workflow (and where it breaks)

A typical literature search today looks like this:

1. Formulate a search query manually on each platform.
2. Browse results in a web UI. Re-formulate. Browse again.
3. Save interesting citations to Zotero or Mendeley.
4. Read abstracts, evaluate relevance by gut feeling.
5. Retrieve full text for promising hits. Annotate.
6. Write a document citing the finds.

This workflow fails on several axes:

- **Not reproducible.** You cannot re-run "the exact search from last Tuesday"
  and get deterministic, auditable results. Search history is ephemeral.
- **Not programmable.** There is no way to script a multi-source search, apply
  filters, and pipe results into downstream analysis — it's all point-and-click.
- **No LLM integration.** Relevance evaluation is manual. LLMs could help
  develop research questions, define search terms, score abstract relevance,
  summarize full papers, and enable conversational exploration of publications —
  but no tool integrates this into a structured, auditable pipeline.
- **No citation chaining.** A rigorous literature search doesn't stop at direct
  hits — it follows references (backward snowballing) and citing papers (forward
  snowballing) to find work the original query missed. Today this is entirely
  manual: open a paper, scan its reference list, repeat. There is no tool that
  automates recursive citation expansion with a configurable blast radius
  (depth) and relevance-gated traversal (only follow citations whose abstracts
  score above a threshold for the research question).
- **No knowledge structure.** Citation networks and concept relationships exist
  implicitly but are not surfaced. There is no way to visualize clustering by
  keyword, analyze citation graphs, or identify hidden connections across
  sub-domains.
- **Not FAIR.** Outputs (citation lists, search parameters, relevance
  judgments) are not structured in machine-readable, interoperable formats.

### The cost of the status quo

- **For individual researchers:** Hours of manual searching per project.
  Inability to systematically explore unfamiliar domains. No audit trail.
- **For systematic literature reviews (SLRs):** A PRISMA/RICO-compliant SLR is
  easily a $100k+ engagement and months of consulting firm labor. The work is
  largely mechanical — search, screen, extract — yet the tooling forces manual
  execution.
- **For research quality:** Without reproducible, high-recall search, literature
  reviews have blind spots. Important papers are missed. New research domains
  are hard to explore because of lack of existing knowledge.

### What happens if we do nothing

Researchers continue doing manual, unreproducible searches. SLRs remain
prohibitively expensive. The gap between what LLMs can do and what researchers
actually use widens. The opportunity to build a local-first, FAIR-compliant,
domain-aware retrieval tool passes to commercial SaaS platforms that prioritize
convenience over transparency and reproducibility.

## Impact

### Who is affected

| Stakeholder | Relationship | Impact |
|---|---|---|
| Domain researchers (radiopharma, PET, detector physics) | Direct users | Hours saved per search, ability to explore new domains, reproducible audit trail |
| SLR practitioners / consulting firms | Direct users (later phase) | Weeks→days reduction in systematic review timelines |
| Regulatory teams needing evidence synthesis | Indirect beneficiaries | Higher-quality, more complete evidence bases |
| Open-science community | Ecosystem beneficiaries | FAIR-compliant tool and outputs raise the bar for reproducibility |

### Severity

- **SLR workflows:** Critical — the current process is prohibitively slow and
  expensive.
- **Day-to-day research:** Moderate — quality-of-life improvement with
  compounding value as the knowledge base grows.

### Success criteria (full vision, across all phases)

1. A researcher can express a multi-source literature search as a reproducible
   command (or script) and get structured, FAIR-compliant output.
2. Successive search runs are auditable and diffable — the user can see what
   changed between runs (new papers, removed papers) while the underlying
   corpus evolves.
3. LLM-assisted relevance scoring produces recall comparable to expert human
   screening (target: ≥90% recall against a human-curated gold standard).
4. Citation chaining (forward and backward snowballing) is automated with
   configurable depth (blast radius) and relevance-gated traversal, so the
   tool follows citations-of-citations only when they meet a relevance
   threshold for the research question.
5. The tool runs locally — no mandatory cloud dependency for core functionality.
6. Research questions are first-class entities with question-specific relevance
   assessments — not just citation counts, but how relevant *this* paper is to
   *that* question.

Phase-specific success criteria are defined in [Proposed Solution](#success-criteria-phase-1) and [Phasing](#phasing).

## Prior Art & References

### Commercial tools

| Tool | What it does | Gap |
|---|---|---|
| [Elicit](https://elicit.com) | AI-powered SLR platform. 138M papers, 96% screening recall, 94% extraction accuracy. Guides users through question refinement → search → screening → extraction → synthesis. | Cloud-only SaaS. Not programmable or scriptable. Prose-heavy UI, not CLI-first. No graph analysis. No local execution. Not FAIR-output oriented. Inspiration for the LLM-assisted relevance pipeline. |
| [Connected Papers](https://www.connectedpapers.com/) | Visual graph exploration of related papers. Uses co-citation + bibliographic coupling (not direct citation trees) to build a similarity graph. Selects ~30 strongest connections from ~50k candidates. Force-directed layout. Backed by Semantic Scholar corpus. Integrated with arXiv. | Cloud-only, closed-source. No CLI, no API, no local execution. No LLM integration. No configurable blast radius. No export beyond limited citation formats. But the **UX and visual approach** — similarity-based clustering, force-directed graph, interactive node exploration — is directly relevant as inspiration for Scitadel's local graph rendering. |
| Zotero / Mendeley | Citation management, annotation, bibliography export. | Not programmable. No federated search. No LLM integration. No knowledge graph. Manual-first. |

### Open-source tools — federated search

| Tool | License | What it does | Gap | Learn from |
|---|---|---|---|---|
| [scholarcli](https://pypi.org/project/scholarcli/) | MIT | CLI for Semantic Scholar, OpenAlex, DBLP, Web of Science, IEEE Xplore, arXiv. Interactive TUI review interface, LLM-assisted classification, session management, note-taking, PDF viewing. JSON/CSV/BibTeX output. Python ≥3.12. | No PubMed, no INSPIRE-HEP. No citation chaining. No knowledge graph. No FAIR output. No versioned data backend. | **Closest UX competitor.** TUI review interface, session management, LLM classification — all directly relevant patterns. Study its architecture. |
| [opencite](https://pypi.org/project/opencite/) | MIT | Unified CLI + Python lib. Searches Semantic Scholar, OpenAlex, PubMed in parallel with deduplication. BibTeX output, citation graph traversal, PDF retrieval, batch downloads. Alpha (v0.2.1). | Early/unstable. No INSPIRE-HEP. No LLM relevance scoring. No knowledge graph. No FAIR. | Parallel multi-source search with dedup is the same core pattern Scitadel needs. Citation graph traversal is partially implemented. |
| [SearchTheScience](https://github.com/philmade/SearchTheScience) | Unknown | Async multi-source search: PubMed, arXiv, OpenAlex, Zenodo, ResearchGate. Pydantic models, smart ranking, deduplication. LLM-friendly. | Very early (3/18 search types working). No citation chaining. No graph. No TUI/CLI. | Async design + Pydantic models for API responses is a good pattern. |
| [docxology/literature](https://github.com/docxology/literature) | Apache-2.0 | Unified search across arXiv, PubMed, OpenAlex, Semantic Scholar, CrossRef, DBLP. PDF management, BibTeX, LLM summaries, meta-analysis (PCA, keyword analysis, temporal trends). | No knowledge graph, no structured relevance pipeline, no FAIR output, no INSPIRE-HEP, no versioned data backend. | Broadest feature set among open-source options. Meta-analysis tools (PCA, temporal trends) are interesting for later phases. |
| [PyAlex](https://github.com/J535D165/pyalex) | MIT | Mature OpenAlex wrapper (348 stars). Pipe operations, semantic search, abstract conversion. v0.21 (Feb 2026). | Single-source only. | Well-designed Python API. Could be used as a dependency rather than reimplemented. |
| [openalexcli](https://pypi.org/project/openalexcli/) | MIT | CLI for OpenAlex. JSON, BibTeX, table output. | Single-source only. | Clean CLI design patterns. |
| [inspy-hep](https://github.com/mhostert/inspy-hep) | MIT | Python wrapper for INSPIRE-HEP API. | Single-source only. | Could be used as a dependency for the INSPIRE adapter. |

### Open-source tools — citation snowballing

| Tool | License | What it does | Gap | Learn from |
|---|---|---|---|---|
| [paperfetcher](https://github.com/paperfetcher/paperfetcher) | MIT | Automated forward/backward snowballing using 1.2B+ DOI citations from Crossref + COCI. Handsearching across 90k+ journals. RIS/CSV/Excel/DataFrame export. Published in *Research Synthesis Methods*. | No relevance gating — fetches all citations at a given depth regardless of relevance. No LLM integration. No graph visualization. | **The reference implementation for snowballing.** Published, peer-reviewed methodology. Scitadel should match its coverage (Crossref/COCI) and add relevance-gated traversal on top. |

### Open-source tools — LLM screening & relevance

| Tool | License | What it does | Gap | Learn from |
|---|---|---|---|---|
| [LatteReview](https://pouriarouzrokh.github.io/LatteReview/) | MIT | Multi-agent SLR framework with local + cloud LLM support. Screening, scoring, RAG. | Focused on screening only. Not a full retrieval + graph + chat pipeline. | Multi-agent architecture, local LLM support, RAG design. |
| [LitLLM](https://litllm.github.io/) | Apache-2.0 | Automated literature review writing using RAG. Keyword + embedding search with LLM re-ranking. | Write-up focused, not retrieval. | LLM re-ranking pipeline: keyword search → embedding retrieval → LLM rerank. Directly applicable to Scitadel's relevance scoring. |
| [AiReview](https://arxiv.org/abs/2504.04193) | Unknown | Open platform for LLM-accelerated SLR. Title/abstract screening. | Web-based. Screening-only. | Screening methodology. |
| [prismAId](https://joss.theoj.org/papers/10.21105/joss.07616) | Unknown | Open-source LLM information extraction for systematic reviews. | Extraction-only. | Structured data extraction patterns. |
| [RankLLM](https://github.com/castorini/rank_llm) | MIT | Modular Python package for LLM-based document reranking. Supports open + proprietary models. Integrates with Pyserini. 568 stars. | Not literature-specific. | Reranking architecture. Could be used as a dependency for relevance scoring. |

### Open-source tools — knowledge graph & paper chat

| Tool | License | What it does | Gap | Learn from |
|---|---|---|---|---|
| [rahulnyk/knowledge_graph](https://github.com/rahulnyk/knowledge_graph) | MIT | Text → knowledge graph using local LLMs (Mistral/Ollama). Concept extraction, community detection, pyvis visualization. 3k stars. | Not literature-specific. No API integrations. | Architecture: chunk → concepts → edges → graph. Local LLM (Ollama) integration. Pyvis for visualization. Directly relevant for later knowledge-graph phase. |
| [researchkg](https://github.com/ps1526/researchkg) | Unknown | Builds knowledge graphs from a DOI, paper title, or concept. JSON export/import. Plans for Graph RAG. | Very early. Limited scope. | DOI → knowledge graph pipeline. |
| [scientific-paper-chat-rag](https://github.com/StadynR/scientific-paper-chat-rag) | Unknown | Local chatbot for PDF papers. Streamlit + LangGraph + Ollama + ChromaDB. MemoRAG, dynamic model selection, source citations with page numbers. | Single-paper focus. No multi-paper corpus Q&A. No federated search. | MemoRAG pattern (memory-augmented retrieval). Source citation with page numbers — essential for trustworthy answers. |
| [ragbase](https://github.com/curiousily/ragbase) | MIT | Local RAG with LangChain + Ollama + Qdrant. Reranking, semantic chunking. 96 stars. | Not literature-specific. | Reranking + semantic chunking pipeline for local RAG. |

### Open-source tools — MCP / agent integration

| Tool | License | What it does | Gap | Learn from |
|---|---|---|---|---|
| [mcsci-hub](https://pypi.org/project/mcsci-hub/) | MIT | MCP server aggregating CrossRef, OpenAlex, Semantic Scholar, Sci-Hub via FastMCP. Parallel search, graceful degradation, citation traversal, BibTeX. | Sci-Hub dependency raises legal concerns. MCP-only (no standalone CLI). | FastMCP-based multi-source aggregation with graceful degradation. Citation traversal. Interesting that the MCP pattern is already being applied to literature search. |
| [academia_mcp](https://github.com/IlyaGusev/academia_mcp) | Unknown | MCP server for ArXiv, ACL Anthology, Hugging Face, Semantic Scholar. Optional LLM analysis. | CS/NLP focused. No PubMed, no physics sources. | MCP server pattern for academic search. |

### Data sources (APIs)

| Source | Coverage | Role in Scitadel |
|---|---|---|
| **PubMed / MEDLINE** | 37M+ biomedical citations | Tier 1 — core biomedical search |
| **arXiv** | 2.4M+ preprints (physics, CS, math, bio) | Tier 1 — preprint search, physics coverage |
| **INSPIRE-HEP** | HEP literature corpus | Tier 1 — detector physics, accelerator science |
| **OpenAlex** | 250M+ works, all disciplines | Tier 1 — broad coverage, open metadata |
| **Crossref** | 170M+ DOI records, funding, licenses | Tier 2 — DOI resolution, metadata enrichment |
| **Semantic Scholar** | 200M+ papers, SPECTER2 embeddings, citation graph | Tier 2 — free embeddings, paper similarity, citation data |
| **Unpaywall** | OA status + links for DOIs | Tier 2 — open-access full-text resolution |
| **Europe PMC** | 33M+ publications, text-mined entities | Tier 3 — broader European biomed coverage, annotations |
| **CORE** | 200M+ OA full-text records | Tier 3 — largest open-access full-text aggregator |

### Technology references

| Technology | Relevance |
|---|---|
| [Dolt](https://dolthub.com) | Git-like versioned SQL database. Branch/merge/diff on data. Feb 2026: Git remotes as Dolt remotes. Candidate for versioned, local-first data backend. |
| [Kuzu](https://kuzudb.com) | Embedded graph database (MIT). Cypher queries, HNSW vector indexing, integrates with NetworkX/DuckDB/LangChain. "SQLite for graphs." Candidate for later-phase graph needs. |
| NetworkX | In-memory Python graph library. Sufficient for citation network analysis up to ~100k nodes. Zero infrastructure. |
| FAIR Principles ([Wilkinson et al. 2016](https://www.nature.com/articles/sdata201618)) | Findable, Accessible, Interoperable, Reusable. Guiding framework for Scitadel's output design. |

## Open Questions

### Assumptions requiring validation

| # | Assumption | Risk | Validation approach |
|---|---|---|---|
| A1 | Researchers want CLI-first tooling | Medium | CLI-first ≠ CLI-only. A TUI or web frontend can layer on later. Target early adopters who value scriptability. |
| A2 | Local LLMs are good enough for relevance scoring | High | Benchmark local models (Mistral, Llama) vs cloud models (GPT-4, Claude) on a curated relevance test set. Hybrid approach likely needed. |
| A3 | Federated search across 4+ APIs is maintainable | Medium | Adapter pattern with per-source abstraction. Each source is isolated. API changes affect only one adapter. |
| A4 | Dolt is the right database backend | Medium | Evaluate Dolt vs SQLite+git vs DuckDB. Abstract the storage layer so the choice is reversible. |
| A5 | A graph DB is needed from day one | Low (validated: no) | Citation network analysis starts with NetworkX or relational tables. Graph DB (Kuzu) is a later-phase enhancement. Vector-based RAG is sufficient for initial Q&A. |
| A6 | The SLR engine (future) won't conflict with Scitadel's design | Medium | Keep Scitadel's output interfaces well-defined and FAIR-compliant. Design for the downstream consumer without coupling to it. |
| A7 | Semantic Scholar SPECTER2 embeddings are sufficient for domain-specific relevance | Medium | Benchmark against expert relevance judgments in radiopharma/PET. May need domain-adapted embeddings later. |
| A8 | Relevance-gated citation chaining is tractable at reasonable depth | Medium | Citation graphs grow exponentially. A paper with 50 references, each with 50 references = 2,500 at depth 2. Relevance gating is essential to prune the tree — but the quality of that gating determines whether the blast radius is useful or noisy. Needs empirical tuning. |

### Risks

| Risk | Severity | Mitigation |
|---|---|---|
| **API instability** — external APIs change schemas, rate-limit, or deprecate | Medium | Per-source adapters, response caching, local result snapshots |
| **LLM cost/quality tradeoff** — cloud models expensive at scale, local models may lack nuance | High | Benchmark per task. Pluggable model backend. Use local for cheap tasks (keyword extraction), cloud for hard tasks (relevance scoring). |
| **Scope creep** — the vision spans search + LLM + graph + chat + write-up | High | Strict phasing. Scitadel core first (federated search + structured output). LLM pipeline second. Graph and chat later. SLR write-up is a separate product. |
| **Solo developer** — bus factor of 1, limited time | High | Ruthlessly small scope per phase. Ship working increments. Open-source early. |
| **Legal / TOS** — scraping full text from publishers may violate terms | Medium | Use only official APIs and open-access content. Respect rate limits. Never cache copyrighted full text without license. Unpaywall for OA resolution. |
| **Graph DB premature complexity** — knowledge graph is appealing but architecturally heavy | Medium | Defer to later phase. Start with citation metadata in relational/tabular form. NetworkX for analysis. Kuzu when graph queries become essential. |
| **Citation chain explosion** — recursive snowballing grows exponentially with depth | Medium | Relevance-gated traversal (only follow edges above a threshold). Configurable max depth (blast radius). Rate-limit awareness per API. Caching to avoid re-fetching already-seen papers. |
| **Transformer attention for relevance** — R&D risk, not a proven production pattern for literature graphs | Low | Classified as exploratory / nice-to-have. Not in scope for any near-term phase. |
| **LLM non-determinism breaks reproducibility** — LLM-augmented pipeline steps (relevance gating, summarization) are inherently non-reproducible | Medium | Log full provenance for every LLM call: prompt, model, temperature, output. Search itself stays deterministic. LLM steps are auditable and re-runnable, not reproducible. Schema anticipates this from Phase 1. |
| **Storage layer lock-in** — tight coupling to SQLite blocks future migration to Dolt | Medium | Repository pattern: all DB access behind Python interface (protocol/abstract class). No raw SQL in business logic. No ORM. Explicit SQL in repository implementations only. Schema as code via SQL migration files. |

### Dependencies (Phase 1)

| Dependency | Type | Stability | Risk |
|---|---|---|---|
| PubMed E-utilities API | External API | Stable, NCBI-maintained | Low — well-documented, long-lived |
| arXiv API | External API | Stable | Low — simple, rarely changes |
| OpenAlex API | External API | Stable, actively developed | Low — generous rate limits, open |
| INSPIRE-HEP API | External API | Stable, CERN-maintained | Low — niche but reliable |
| PyAlex (Python library) | OSS dependency | Mature (v0.21, MIT) | Low |
| httpx (Python library) | OSS dependency | Mature, widely used | Low |
| bibtexparser (Python library) | OSS dependency | Mature | Low |
| Typer or Click (Python library) | OSS dependency | Mature, widely used | Low |

### Envisioned end-to-end workflow

The full pipeline Scitadel aims to enable (across phases):

1. **Develop research question** — with LLM assistance
2. **LLM defines search terms** — translating the research question into
   structured queries per source
3. **Federated search** — query all sources (PubMed, arXiv, INSPIRE-HEP,
   OpenAlex, etc.) in parallel, deduplicate, merge
4. **LLM evaluates relevance** — score abstracts of results against the
   research question
5. **Citation chaining** — follow references/citations of relevant hits,
   relevance-gated, configurable blast radius
6. **Full-text retrieval & summarization** — for high-relevance papers, fetch
   full text (OA only), summarize with LLM
7. **Interactive exploration** — "chat with" individual publications or the
   corpus; ask questions across the retrieved set
8. **Knowledge graph** — visualize citation networks, concept clusters,
   similarity graphs (Connected Papers–style, locally rendered)
9. **Structured output** — BibTeX, FAIR-compliant metadata, structured evidence
   tables, exportable graph data
10. **Report generation** — (future SLR engine) Typst-based write-up with
    pre-defined plots, PRISMA flow diagrams, citing retrieved publications

All steps persist their outputs to a versioned, local-first data backend for
reproducibility and auditability.

### Two-layer architecture consideration

Scitadel is envisioned as the **exploratory retrieval engine**. A future **SLR
engine** would consume Scitadel's outputs and add structured write-up (e.g.,
Typst templates, PRISMA flow diagrams, pre-defined plots). This separation is
important:

- Scitadel must not make design decisions that block the SLR engine.
- Scitadel's outputs must be well-defined, FAIR-compliant, and
  machine-readable so downstream consumers can build on them.
- The SLR engine may impose requirements on Scitadel (e.g., provenance
  tracking, structured evidence tables) that should be anticipated but not
  prematurely implemented.

## Proposed Solution

### Approach: Modular Python Library + CLI

Scitadel is a **Python library with a CLI entry point**. Each capability is a
composable module: source adapters, search orchestration, result
merging/deduplication, structured output. The CLI is a thin wrapper over the
library API.

- **Library-first** ensures programmability and reproducibility (core goals).
- **CLI** gives researchers immediate scriptable access.
- Any downstream interface (TUI, web UI, MCP server) can import and use the
  library without redesign.

### MVP scope (Phase 1): Federated Search + Structured Output

| # | Capability | Detail |
|---|---|---|
| 1 | **Source adapters** | PubMed, arXiv, OpenAlex, INSPIRE-HEP (Tier 1 sources) |
| 2 | **Federated search** | Parallel query across selected sources, configurable per search |
| 3 | **Deduplication & merge** | DOI-based exact match + fuzzy title matching. Merge metadata from multiple sources into canonical records |
| 4 | **Local-first DB** | SQLite as canonical store behind an abstracted repository layer (see [Storage architecture](#storage-architecture)). All results, search params, metadata persisted |
| 5 | **Structured export** | BibTeX, JSON, CSV as DB-backed projections only (exports read persisted records, not transient in-memory adapter responses) |
| 6 | **Reproducible & auditable search** | Search parameters + raw results stored per run. Re-runnable. Successive runs can be diffed to show new/removed papers. Architecture anticipates logging non-deterministic (LLM) steps with full provenance (prompt, model, temperature, output) in Phase 2 |
| 7 | **CLI** | `scitadel search`, `scitadel export`, `scitadel history` style commands |
| 8 | **Python library API** | All functionality importable. CLI is a thin wrapper |
| 9 | **Agent-consumable output** | Clean export formats (markdown, structured JSON) designed for external LLM agents (Cursor, Claude Code) to consume for relevance scoring, summarization |

### Out of scope (deferred to later phases)

| # | Capability | Why deferred | Target phase |
|---|---|---|---|
| 10 | LLM-assisted relevance scoring (built-in) | Requires model benchmarking (A2), adds complexity. Bootstrapped via external agent in MVP | Phase 2 |
| 11 | LLM search term generation (built-in) | Bootstrapped via Cursor agent skills in MVP | Phase 2 |
| 12 | Citation chaining (snowballing) | Depends on solid search + dedup foundation | Phase 2 |
| 13 | Tier 2/3 sources (Crossref, Semantic Scholar, Unpaywall, Europe PMC, CORE) | Add incrementally after Tier 1 is solid | Phase 2+ |
| 14 | Full-text retrieval & transformation | Legal complexity, PDF handling, extraction pipeline | Phase 2–3 |
| 15 | Chat with papers / corpus Q&A | Requires RAG infrastructure | Phase 3+ |
| 16 | Knowledge graph / visualization | Requires graph analysis, UI layer | Phase 3+ |
| 17 | TUI / Web UI | Layer on top of library after core is stable | Phase 3+ |
| 18 | MCP server | Thin interface layer when library is stable | Future |
| 19 | SLR engine / report generation | Separate product consuming Scitadel outputs | Future |

### Data model

The data model has five core entities that capture the key insight: **a paper's
relevance is not a property of the paper — it's a property of the paper in the
context of a specific research question.** This question-specific weighting is
Scitadel's differentiator.

| Entity | Purpose | MVP status |
|---|---|---|
| `papers` | Canonical, deduplicated paper records. A paper exists once regardless of how many searches found it. Stores metadata, abstract, DOIs, source-specific IDs | Populated |
| `searches` | Immutable search run records. Stores query, sources, date, parameters. Links to papers found (many-to-many via `search_results`) | Populated |
| `search_results` | Join table: search → paper. Stores per-source rank, score, raw response metadata | Populated |
| `research_questions` | First-class entities. Well-crafted research questions, versioned, linked to search term combinations. In MVP, created manually or via Cursor agent skill; in Phase 2, with built-in LLM assistance | Populated |
| `search_terms` | Keyword combinations linked to a research question. Generated by agent skill or manually. Picked up by `scitadel search` | Populated |
| `assessments` | Paper × research question → relevance score, reasoning, provenance (model, prompt, temperature, timestamp). Multiple assessments per paper (different questions, different models, human override) | Schema exists, populated externally (agent) in MVP, built-in in Phase 2 |
| `extractions` | Transformed paper content (markdown, structured JSON). Stores tool, tool version, format, content as TEXT, created_at. Multiple rows per paper for version comparison | Schema exists, populated in Phase 2–3 |

### Storage architecture

**Design constraint:** The storage layer is abstracted behind a repository
interface (Python protocol / abstract class) so the DB engine is swappable
without touching business logic.

- **Repository pattern** — all DB access goes through `PaperRepository`,
  `SearchRepository`, `AssessmentRepository`, etc. No raw SQL in business
  logic.
- **SQLite implementation** — the MVP concrete implementation. Zero
  infrastructure, ships with Python.
- **Dolt implementation** — future drop-in replacement. Dolt speaks MySQL wire
  protocol; the implementation uses a MySQL-compatible driver, same interface.
  Enables git-style branch/merge/diff on data (e.g., diff extraction content
  across tool versions for the entire corpus).
- **No ORM** — a thin repository layer with explicit SQL is more portable and
  easier to swap than an ORM that leaks engine-specific behavior.
- **Schema as code** — migrations defined in SQL files, applied by the
  repository layer.

**File storage:**

- **Extracted text** (markdown, structured JSON from PDF transformation) is
  stored **in the DB** (`extractions` table). A typical paper extraction is
  10–150 KB; 10k papers ≈ 1 GB — well within SQLite limits. This enables
  atomic transactions, full-text search (FTS5), and versioned comparison of
  extraction outputs. With Dolt, this becomes diffable across the corpus.
- **Original PDFs** are stored on the **filesystem** (`~/.scitadel/originals/`)
  with paths tracked in the DB. PDFs are large (1–10 MB), binary, and not
  queryable — filesystem storage avoids DB bloat and allows streaming access.

```text
~/.scitadel/
  scitadel.db                ← SQLite: metadata, extractions, assessments
  originals/
    <paper-id>.pdf           ← raw PDFs (Phase 2–3, OA only)
```

### Build vs buy

| Component | Decision | Rationale |
|---|---|---|
| PubMed adapter | **Build** | Direct E-utilities API. Straightforward, well-documented |
| arXiv adapter | **Build** | Simple Atom/XML API. Existing wrappers are too thin to justify a dependency |
| OpenAlex adapter | **Use [PyAlex](https://github.com/J535D165/pyalex)** | Mature (v0.21, 348 stars, MIT). Well-designed API. No reason to reimplement |
| INSPIRE-HEP adapter | **Evaluate [inspy-hep](https://github.com/mhostert/inspy-hep)**; build if too limited | MIT. Evaluate fit before committing |
| Deduplication | **Build** | DOI matching is trivial. Fuzzy title matching needs project-specific tuning |
| Local DB | **Use SQLite** via Python stdlib `sqlite3` | Zero infrastructure. Abstract behind repository layer for future Dolt swap |
| CLI framework | **Use [Typer](https://typer.tiangolo.com/) or [Click](https://click.palletsprojects.com/)** | Standard Python CLI libraries |
| Structured output | **Build** + **use [bibtexparser](https://bibtexparser.readthedocs.io/)** | BibTeX via library. JSON/CSV via stdlib |
| Async HTTP | **Use [httpx](https://www.python-httpx.org/)** | Async-capable HTTP client for parallel source queries |

### LLM integration strategy (MVP bootstrapping)

Scitadel's MVP does **not** embed LLM calls. Instead, LLM-assisted workflows
are bootstrapped via external agents:

1. **Research question formulation** — a Cursor agent skill guides the
   researcher through crafting a well-formed research question and saves it to
   the DB.
2. **Search term generation** — a second agent skill translates the research
   question into keyword combinations, saved to the DB, picked up by
   `scitadel search`.
3. **Relevance scoring** — Scitadel exports papers (metadata, abstracts, or
   full-text extractions) in agent-consumable formats. An external agent
   (Cursor, Claude Code) receives these + the research question and scores
   relevance. Results are written back to the `assessments` table.

This decoupling keeps Scitadel deterministic and testable while enabling
LLM-powered workflows from day one. Built-in LLM integration follows in
Phase 2.

### Success criteria (Phase 1)

| # | Criterion | Measurable |
|---|---|---|
| S1 | A researcher can run `scitadel search "PET tracer radiopharma" --sources pubmed,arxiv,openalex,inspire` and get deduplicated, merged results in the local DB | Integration test |
| S2 | Re-running a search stores a new result set. Two runs can be diffed to show new/removed papers. Search parameters are fully auditable | Diff test |
| S3 | Results export to BibTeX, JSON, CSV with complete metadata | Output validation |
| S4 | Exported markdown/JSON is clean enough for an external LLM agent to rank relevance | Manual validation with Cursor/Claude |
| S5 | Deduplication correctly merges the same paper found across multiple sources (e.g., arXiv + PubMed cross-listed paper) | Test with known cross-listed papers |
| S6 | Search history is queryable: `scitadel history` shows past searches and parameters | CLI test |
| S7 | The library API supports the same operations as the CLI, importable from Python | Unit/integration tests |
| S8 | Research questions and search terms are first-class DB entities, linkable to searches and assessments | Schema/integration test |

### Feasibility assessment

**Technical:** All Tier 1 APIs are public and documented. Python ecosystem has
mature tools for every component. No novel technology risk.

**Resource:** Solo developer, Python expertise. MVP scope is a focused library +
CLI — no UI, no LLM infra, no graph DB. Estimated effort: **4–6 weeks**.

**Dependencies:** All four APIs are stable and free. Rate limits are manageable
(PubMed: 10 req/s with API key; arXiv: modest; OpenAlex: generous with polite
pool; INSPIRE-HEP: generous). No paid dependencies.

**Key risk:** Storage layer decision (SQLite vs Dolt). Mitigated by starting
with SQLite behind an abstraction; evaluate Dolt when branch/merge/diff on data
becomes a concrete need.

## Alternatives Considered

### A. MCP Server–first (Agent-first)

Build Scitadel primarily as an MCP server exposing literature search as tools
for AI agents (Cursor, Claude, etc.).

**Rejected because:** MCP-only limits non-agent use. Would still need a CLI for
reproducible scripting. Risks coupling to a protocol that's still maturing. Can
be added as a thin layer over the library later when the library is stable.

### B. Full-stack application (TUI/Web UI–first)

Build a TUI or web application with integrated search, screening, and
visualization from the start.

**Rejected because:** Slower to ship, higher risk of scope creep, harder to
maintain solo. Contradicts the "ruthlessly small scope per phase" risk
mitigation. A UI is a future layer on top of the library, not a starting point.

### C. Adopt an existing tool

Use scholarcli, opencite, or another existing tool as the foundation.

**Rejected because:** No existing tool covers all four Tier 1 sources, provides
question-specific relevance assessments as a core concept, or offers a
versioned local-first data backend with the repository abstraction needed for
future Dolt migration. The closest tools (scholarcli, opencite) are useful as
architectural references but not as foundations.

### D. Use an ORM (SQLAlchemy, etc.) for DB abstraction

Use an ORM instead of a hand-rolled repository pattern for DB engine
portability.

**Rejected because:** ORMs tend to leak engine-specific behavior and generate
SQL that's hard to port. SQLite → Dolt (MySQL wire protocol) is a known future
migration path; a thin repository layer with explicit SQL is more transparent
and easier to swap than an ORM abstraction.

## Phasing

### Phase 1 — MVP: Federated Search + Structured Output

- **Scope:** Library + CLI, 4 source adapters, dedup, SQLite DB with repository
  abstraction, structured export, research questions + search terms as
  first-class entities, agent-consumable output
- **Deliverables:** Installable Python package, CLI commands (`search`,
  `export`, `history`), importable library API
- **Success criteria:** S1–S8 above
- **Effort:** ~4–6 weeks

### Phase 2 — LLM Integration + Citation Chaining

- **Scope:**
  - Built-in LLM relevance scoring (beyond bootstrapped agent approach)
  - Provenance logging for all non-deterministic steps (prompt, model,
    temperature, output)
  - Citation chaining — forward/backward snowballing with relevance-gated
    traversal and configurable blast radius
  - Full-text retrieval for OA papers (via Unpaywall) and pluggable extraction
    pipeline (PDF → markdown/JSON, multiple tools, stored in DB)
  - Tier 2 sources: Crossref, Semantic Scholar
  - Agent skills for research question formulation and search term generation
- **Depends on:** Phase 1 complete, LLM benchmarking (assumption A2)
- **Success criteria:** Relevance scoring recall ≥90% vs human gold standard;
  snowballing produces relevant discoveries beyond direct search hits;
  extraction provenance enables re-extraction when tools improve

### Phase 3 — Knowledge Graph + Chat + UI

- **Scope:**
  - Citation network visualization (NetworkX initially; Kuzu if graph queries
    become essential)
  - Connected Papers-style runtime rendering via a dedicated graph UI stack
    (e.g., pyvis/cytoscape/d3). Mermaid remains documentation-only.
  - Chat with papers / corpus Q&A (RAG over extracted text)
  - TUI or web UI layered on library
  - Tier 3 sources (Europe PMC, CORE)
- **Depends on:** Phase 2 complete

### Future — SLR Engine (separate product)

- Typst-based report generation, PRISMA flow diagrams, pre-defined plots
- Consumes Scitadel's FAIR-compliant outputs
- Separate repository, separate lifecycle

## Implementation Tracking

- Epic: [#1](https://github.com/vig-os/scitadel/issues/1)
- Milestone: Phase 1: MVP
- Phase 1 implementation issues: #2, #3, #4, #5, #6, #7, #8, #9, #10, #11, #12, #13, #14

## Decision

Approved to proceed with architecture and implementation planning. Tracking has
been created in GitHub under the Phase 1 milestone and linked epic/issues.
