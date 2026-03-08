# DES-002: Scitadel Rust Rewrite — Development Plan

| Field       | Value                                                        |
|-------------|--------------------------------------------------------------|
| **Status**  | draft                                                        |
| **Authors** | Lars Gerchow                                                 |
| **Created** | 2026-03-08                                                   |
| **Updated** | 2026-03-08                                                   |
| **RFC**     | `docs/rfcs/RFC-001-2026-02-26-scitadel-problem-space.md`     |
| **Supersedes** | `docs/designs/DES-001-2026-02-26-scitadel-phase1-architecture.md` (Python) |

---

## 1. Vision

Scitadel becomes a full Rust rewrite of the Python prototype, expanding from a
CLI literature search engine into a **product suite** that replaces Zotero for
researchers who value programmability, AI integration, and reproducibility.

**Product surfaces:**

| Surface | Description | Phase |
|---|---|---|
| `scitadel` (CLI) | Clap-based terminal commands | 1 |
| `scitadel-mcp` | MCP server for AI agent integration (rmcp) | 1 |
| `scitadel-tui` | Ratatui terminal dashboard | 2 |
| `scitadel-typst` | WASM plugin for live bibliography in Typst | 3 |
| `scitadel-desktop` | Tauri v2 standalone app (Zotero replacement UX) | 4 |
| `scitadel-web` | Self-hosted Axum web app with Shibboleth auth | 5 |

**Data backbone:**

- **Local**: SQLite (embedded, offline-first, zero-infra)
- **Hub**: Dolt (PostgreSQL wire protocol, git-for-data, collaboration)
- **Sync**: SQLite ↔ Dolt bidirectional merge daemon

---

## 2. Principles

### 2.1 Architecture Principles

| # | Principle | Rationale |
|---|---|---|
| P1 | **Hexagonal architecture** — all external I/O behind trait boundaries | Enables testing with mocks, swapping SQLite↔Dolt, replacing adapters |
| P2 | **Local-first, sync-second** — every operation works offline against SQLite; Dolt sync is additive | Researchers work on planes, in labs, in restricted networks |
| P3 | **Immutable audit trail** — searches, assessments, and citations are append-only events | Reproducibility is a core differentiator; never silently mutate history |
| P4 | **Single binary** — CLI, MCP server, and TUI compile into one binary with subcommands | `cargo install scitadel` gives you everything; no runtime dependencies |
| P5 | **FAIR outputs** — all exports are machine-readable, structured, and provenance-tracked | Downstream consumers (SLR engine, Typst, other tools) can build on outputs |
| P6 | **Workspace crate separation** — each concern is its own crate with explicit public API | Compile-time enforcement of module boundaries; parallel compilation |
| P7 | **No ORM** — explicit SQL behind repository traits | Portable across SQLite/Dolt, inspectable, no magic |

### 2.2 Engineering Principles

| # | Principle | Rationale |
|---|---|---|
| E1 | **Type-driven design** — use newtypes, enums, and the type system to make illegal states unrepresentable | `PaperId(Uuid)` not `String`; `SearchStatus::Complete` not `status: &str` |
| E2 | **Error types per crate** — `thiserror` for library crates, `anyhow` at binary entry points | Clean error propagation without stringly-typed errors |
| E3 | **Structured logging everywhere** — `tracing` spans with `search_id`, `source`, `paper_id` correlation | Debuggable async pipelines; export-ready for OpenTelemetry later |
| E4 | **Contract tests against live APIs** — gated behind `#[cfg(feature = "contract-tests")]` | Catch API drift early without blocking CI |
| E5 | **Snapshot tests for serialization** — `insta` for JSON/BibTeX output stability | Prevent accidental format regressions |
| E6 | **Deterministic where possible, auditable where not** — search is deterministic; LLM calls log full provenance | Clear line between reproducible and non-reproducible steps |
| E7 | **Feature flags for heavy dependencies** — Tauri, Dolt, LLM scoring are optional features | Core stays lean; users opt into what they need |

### 2.3 Testing Principles

| # | Principle | Rationale |
|---|---|---|
| T1 | **Test at the boundary** — repository trait impls get integration tests against real SQLite; adapters get contract tests against real APIs | Mocking HTTP is fragile; test the real integration, gate it in CI |
| T2 | **Property-based tests for dedup** — use `proptest` to generate title pairs and verify fuzzy matching invariants | Dedup correctness is critical and edge-case-heavy |
| T3 | **Snapshot tests for all export formats** — `insta` snapshots for BibTeX, JSON, CSV, Typst bibliography | Format stability is a user-facing contract |
| T4 | **Testcontainers for Dolt** — spin up Dolt in Docker for sync integration tests | Real database, no mocks, reproducible in CI |
| T5 | **TUI tests via `ratatui` test backend** — headless rendering assertions | No manual visual testing required |

---

## 3. Technology Stack

### 3.1 Core Dependencies

| Concern | Crate | Version | Why this one |
|---|---|---|---|
| Async runtime | `tokio` | 1.x | Ecosystem standard; required by reqwest, sqlx, rmcp, axum |
| HTTP client | `reqwest` | 0.12+ | Async, connection pooling, gzip, cookie jar for publisher access |
| CLI | `clap` | 4.x (derive) | Best-in-class; subcommands, completions, man pages |
| TUI | `ratatui` + `crossterm` | latest | Active community, flexible widget system |
| MCP server | `rmcp` | latest | Confirmed working; Rust-native MCP implementation |
| SQLite | `rusqlite` + `r2d2` | latest | Embedded, WAL mode, connection pooling |
| PostgreSQL/Dolt | `sqlx` | 0.8+ | Compile-time query checking, async, postgres wire |
| Serialization | `serde` + `serde_json` | 1.x | Universal Rust serialization |
| XML parsing | `quick-xml` + `serde` | latest | PubMed E-utilities (XML), arXiv Atom feeds |
| BibTeX | Custom parser/writer | — | Small surface; own it for full control |
| Fuzzy matching | `strsim` | latest | Jaccard, Jaro-Winkler, Levenshtein |
| UUID | `uuid` | 1.x | Paper IDs, search IDs, question IDs |
| Date/time | `chrono` | 0.4+ | Timestamps with timezone awareness |
| Tracing | `tracing` + `tracing-subscriber` | latest | Structured async-aware logging |
| Error handling | `thiserror` (lib) + `anyhow` (bin) | latest | Typed library errors, ergonomic binary errors |
| Testing | `insta` + `wiremock` + `proptest` + `testcontainers` | latest | Snapshots, HTTP mocks, property tests, Docker |
| Desktop | `tauri` | 2.x | Rust backend, webview frontend, cross-platform |
| Web server | `axum` + `tower` | 0.7+ | Tokio-native, composable middleware |
| Auth (SAML) | `samael` or custom | — | Shibboleth/OpenAthens SAML 2.0 |
| WASM (Typst) | `wasm-minimal-protocol` | — | Typst's plugin interface |

### 3.2 Dev Tooling

| Tool | Purpose |
|---|---|
| `cargo-nextest` | Fast parallel test runner with better output |
| `cargo-llvm-cov` | Code coverage |
| `cargo-deny` | License and vulnerability auditing |
| `cargo-machete` | Detect unused dependencies |
| `cargo-release` | Versioned releases |
| `taplo` | TOML formatting |
| `just` | Task runner (replaces Makefile) |
| `sqlx-cli` | Database migration management |
| `cross` | Cross-compilation for release binaries |
| `oranda` or `mdbook` | Documentation site |
| GitHub Actions | CI/CD |

### 3.3 Infrastructure (for web/hub deployment)

| Component | Technology |
|---|---|
| Container runtime | Docker / Podman |
| Dolt server | `dolt sql-server` (PostgreSQL mode) |
| Object storage | MinIO (self-hosted S3) for PDFs |
| Reverse proxy | Caddy or Traefik (auto-TLS) |
| Auth proxy | Shibboleth SP or `mod_auth_openidc` |
| Monitoring | Prometheus + Grafana (optional) |

---

## 4. Workspace Structure

```
scitadel/
├── Cargo.toml                          # [workspace]
├── justfile                            # task runner
├── deny.toml                           # cargo-deny config
├── .github/
│   └── workflows/
│       ├── ci.yml                      # lint + test + coverage
│       ├── release.yml                 # cross-compile + publish
│       └── contract-tests.yml          # weekly live API tests
│
├── crates/
│   ├── scitadel-core/                  # domain models, traits (ports), service logic
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── models/                 # Paper, Search, Assessment, Question, Citation
│   │   │   │   ├── mod.rs
│   │   │   │   ├── paper.rs
│   │   │   │   ├── search.rs
│   │   │   │   ├── assessment.rs
│   │   │   │   ├── question.rs
│   │   │   │   └── citation.rs
│   │   │   ├── ports/                  # trait definitions (repository, adapter, scorer)
│   │   │   │   ├── mod.rs
│   │   │   │   ├── repository.rs
│   │   │   │   ├── adapter.rs
│   │   │   │   └── scorer.rs
│   │   │   ├── services/               # orchestrator, dedup, export, snowball, scoring
│   │   │   │   ├── mod.rs
│   │   │   │   ├── orchestrator.rs
│   │   │   │   ├── dedup.rs
│   │   │   │   ├── snowball.rs
│   │   │   │   └── scoring.rs
│   │   │   └── error.rs
│   │   └── Cargo.toml
│   │
│   ├── scitadel-db/                    # SQLite + Dolt repository implementations
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── sqlite/                 # rusqlite implementations
│   │   │   │   ├── mod.rs
│   │   │   │   ├── papers.rs
│   │   │   │   ├── searches.rs
│   │   │   │   ├── assessments.rs
│   │   │   │   ├── questions.rs
│   │   │   │   ├── citations.rs
│   │   │   │   └── migrations.rs
│   │   │   ├── dolt/                   # sqlx postgres implementations (feature-gated)
│   │   │   │   ├── mod.rs
│   │   │   │   └── ...
│   │   │   └── error.rs
│   │   ├── migrations/                 # SQL migration files (shared schema)
│   │   │   ├── 001_initial.sql
│   │   │   ├── 002_citations.sql
│   │   │   ├── 003_full_text.sql
│   │   │   ├── 004_collections_tags.sql
│   │   │   └── 005_notes_annotations.sql
│   │   └── Cargo.toml
│   │
│   ├── scitadel-adapters/              # source adapters
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── pubmed.rs
│   │   │   ├── arxiv.rs
│   │   │   ├── openalex.rs
│   │   │   ├── inspire.rs
│   │   │   ├── crossref.rs            # Tier 2
│   │   │   ├── semantic_scholar.rs    # Tier 2
│   │   │   └── unpaywall.rs           # OA full-text resolution
│   │   └── Cargo.toml
│   │
│   ├── scitadel-scoring/               # LLM relevance scoring
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── claude.rs              # Claude API scorer
│   │   │   ├── ollama.rs              # Local LLM scorer (feature-gated)
│   │   │   └── provenance.rs          # Full audit trail for LLM calls
│   │   └── Cargo.toml
│   │
│   ├── scitadel-export/                # export formats
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── bibtex.rs
│   │   │   ├── json.rs
│   │   │   ├── csv.rs
│   │   │   └── typst_bib.rs           # Typst-native bibliography format
│   │   └── Cargo.toml
│   │
│   ├── scitadel-sync/                  # SQLite ↔ Dolt sync
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── changeset.rs           # delta computation
│   │   │   ├── merge.rs               # conflict resolution
│   │   │   └── daemon.rs              # background sync loop
│   │   └── Cargo.toml
│   │
│   ├── scitadel-mcp/                   # MCP server
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   └── tools.rs               # tool definitions + handlers
│   │   └── Cargo.toml
│   │
│   ├── scitadel-tui/                   # ratatui terminal UI
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── app.rs
│   │   │   ├── views/                 # search, library, paper detail, chat
│   │   │   ├── widgets/               # results table, citation tree, chat
│   │   │   └── events.rs
│   │   └── Cargo.toml
│   │
│   ├── scitadel-desktop/               # Tauri v2 desktop app
│   │   ├── src-tauri/
│   │   │   ├── src/
│   │   │   │   ├── main.rs
│   │   │   │   ├── commands.rs        # Tauri IPC → scitadel-core
│   │   │   │   └── state.rs
│   │   │   └── Cargo.toml
│   │   ├── src/                        # frontend (Svelte or Leptos)
│   │   └── tauri.conf.json
│   │
│   ├── scitadel-web/                   # self-hosted web app
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── routes/                # API + page handlers
│   │   │   ├── auth/                  # Shibboleth SAML + standalone JWT
│   │   │   ├── middleware/            # auth, rate limiting, logging
│   │   │   └── ws.rs                  # WebSocket for chat/streaming
│   │   └── Cargo.toml
│   │
│   └── scitadel-typst/                 # WASM Typst plugin
│       ├── src/
│       │   ├── lib.rs                 # wasm-minimal-protocol entry
│       │   └── bib_reader.rs          # SQLite → citation resolution
│       └── Cargo.toml
│
├── xtask/                              # cargo xtask for build automation
│   ├── src/main.rs
│   └── Cargo.toml
│
└── docs/
    ├── rfcs/
    ├── designs/
    └── architecture/
```

---

## 5. Phases

### Phase 0 — Scaffold & Foundation

**Goal:** Compilable workspace with CI, domain models, and SQLite persistence.
No external API calls yet. This phase establishes the skeleton that all future
work builds on.

**Deliverables:**

| # | Deliverable | Crate | Detail |
|---|---|---|---|
| 0.1 | Workspace scaffold | root | `Cargo.toml` workspace, `justfile`, `deny.toml`, CI workflow, `.github/`, `xtask/` |
| 0.2 | Domain models | `scitadel-core` | `Paper`, `SearchRun`, `SearchResult`, `CandidatePaper`, `Assessment`, `ResearchQuestion`, `SearchTerm`, `Citation`, `SnowballRun` — all with newtypes for IDs |
| 0.3 | Port traits | `scitadel-core` | `SourceAdapter`, `PaperRepository`, `SearchRepository`, `AssessmentRepository`, `QuestionRepository`, `CitationRepository` |
| 0.4 | Service skeletons | `scitadel-core` | `SearchOrchestrator`, `DedupService`, `ExportService` — compiling with `todo!()` bodies |
| 0.5 | SQLite persistence | `scitadel-db` | Migrations 001–003 (reuse from Python), `rusqlite` repository impls |
| 0.6 | Error types | all crates | `thiserror` enums per crate |
| 0.7 | Tracing setup | `scitadel-core` | `tracing` integration with `search_id` spans |

**Tests:**

| Test type | What | How |
|---|---|---|
| Unit | Model construction, validation, newtype conversions | `#[test]` in `models/` |
| Unit | Dedup matching logic (DOI exact, title fuzzy) | `#[test]` + `proptest` for edge cases |
| Integration | SQLite repository CRUD against real DB | `rusqlite::Connection::open_in_memory()` |
| Integration | Migration idempotency — run migrations twice, assert no error | In-memory SQLite |
| Snapshot | Model serialization to JSON | `insta` |
| CI | `cargo clippy`, `cargo fmt --check`, `cargo deny check`, `cargo test` | GitHub Actions |

**Exit criteria:**
- `cargo build` compiles all crates
- `cargo test` passes with ≥80% coverage on `scitadel-core` and `scitadel-db`
- CI green: lint, format, deny, test

---

### Phase 1 — Federated Search & CLI

**Goal:** A working `scitadel search` command that queries 4 sources in
parallel, deduplicates, persists to SQLite, and exports BibTeX/JSON/CSV. Plus
MCP server with equivalent capabilities. Feature parity with the Python
prototype's Phase 1.

**Deliverables:**

| # | Deliverable | Crate | Detail |
|---|---|---|---|
| 1.1 | PubMed adapter | `scitadel-adapters` | E-utilities `esearch` + `efetch`, XML parsing, rate limiting (3 req/s default, 10 with API key) |
| 1.2 | arXiv adapter | `scitadel-adapters` | Atom feed search, `quick-xml` parsing |
| 1.3 | OpenAlex adapter | `scitadel-adapters` | REST API, polite pool with email header, pagination |
| 1.4 | INSPIRE-HEP adapter | `scitadel-adapters` | REST API, HEP-specific metadata mapping |
| 1.5 | Search orchestrator | `scitadel-core` | Parallel `tokio::JoinSet`, partial-failure tolerance, timeout per adapter, structured run metadata |
| 1.6 | Dedup & canonicalization | `scitadel-core` | DOI exact → fuzzy title (Jaccard ≥0.85) → source metadata merge. Deterministic, versioned rules |
| 1.7 | Export service | `scitadel-export` | BibTeX writer, JSON (serde), CSV. All DB-backed — never export transient data |
| 1.8 | CLI commands | `scitadel-cli` | `search`, `history`, `show`, `export`, `diff`, `init`, `question`, `terms` |
| 1.9 | MCP server | `scitadel-mcp` | rmcp-based, tools: `search`, `list_searches`, `get_papers`, `export`, `create_question`, `add_search_terms`, `assess_paper` |
| 1.10 | Config | `scitadel-core` | Env-based config: API keys, email, timeouts, DB path, workspace detection |
| 1.11 | Research questions | `scitadel-core` | CRUD for research questions and linked search terms; question-driven search |

**Tests:**

| Test type | What | How |
|---|---|---|
| Unit | Each adapter's response normalization (XML/JSON → `CandidatePaper`) | Fixture files with recorded API responses |
| Unit | Dedup rules: DOI match, fuzzy title, metadata merge priority | `proptest` for title pairs; hand-crafted edge cases |
| Unit | BibTeX/JSON/CSV formatting | `insta` snapshots |
| Integration | Full search pipeline: adapter → dedup → persist → export | In-memory SQLite, `wiremock` for HTTP |
| Integration | CLI e2e: invoke `scitadel search`, check DB state and stdout | `assert_cmd` + `predicates` |
| Integration | MCP tool dispatch: send tool call, verify DB mutation and response | rmcp test client |
| Contract | Live API calls to each source (gated: `--features contract-tests`) | Real HTTP, recorded + compared to fixture |
| Snapshot | Export format stability | `insta` for BibTeX, JSON, CSV |
| CI | All of the above + coverage report | `cargo-nextest` + `cargo-llvm-cov` |

**Exit criteria:**
- `scitadel search "PET tracer" --sources pubmed,arxiv,openalex,inspire` returns deduplicated results
- `scitadel export --format bibtex` produces valid BibTeX
- `scitadel history` and `scitadel diff` work
- MCP server responds to all registered tools
- ≥85% test coverage on `scitadel-core`, `scitadel-adapters`, `scitadel-export`

---

### Phase 2 — LLM Scoring, Snowballing & TUI

**Goal:** Built-in LLM relevance scoring with full provenance, citation chaining
(forward + backward snowballing with relevance-gated traversal), and a ratatui
terminal dashboard for interactive exploration.

**Deliverables:**

| # | Deliverable | Crate | Detail |
|---|---|---|---|
| 2.1 | Claude API scorer | `scitadel-scoring` | `reqwest` to Claude API, structured 0.0–1.0 scoring with rubric, batch concurrency control |
| 2.2 | Ollama scorer | `scitadel-scoring` | Feature-gated local LLM scoring via Ollama HTTP API |
| 2.3 | Provenance logging | `scitadel-scoring` | Every LLM call logs: prompt, model, temperature, raw response, parsed score, reasoning, timestamp |
| 2.4 | OpenAlex citation fetcher | `scitadel-adapters` | `references` and `cited_by` traversal via OpenAlex works API |
| 2.5 | Snowball service | `scitadel-core` | Forward/backward snowballing, configurable depth (1–3), relevance-gated traversal (only follow papers above threshold), dedup against existing corpus |
| 2.6 | Full-text retrieval | `scitadel-adapters` | Unpaywall integration for OA PDF URL resolution |
| 2.7 | PDF extraction | `scitadel-core` | PDF → markdown via `pdf-extract` or external tool; stored in `extractions` table |
| 2.8 | Tier 2 adapters | `scitadel-adapters` | Crossref (DOI enrichment, metadata), Semantic Scholar (embeddings, citation data) |
| 2.9 | TUI app | `scitadel-tui` | Tabs: Search browser, Paper library, Research questions, Citation tree, AI assistant chat |
| 2.10 | Chat engine | `scitadel-core` | Tool-augmented conversation with Claude — can invoke search, score, snowball, export during chat |
| 2.11 | CLI extensions | `scitadel-cli` | `assess`, `snowball`, `tui`, `chat` subcommands |
| 2.12 | MCP extensions | `scitadel-mcp` | Tools: `snowball_search`, `get_full_text`, `assess_batch`, `chat` |

**Tests:**

| Test type | What | How |
|---|---|---|
| Unit | Score parsing, provenance struct construction | Fixture LLM responses |
| Unit | Snowball traversal logic: depth limiting, relevance gating, cycle detection | Mock `CitationRepository` + mock scorer |
| Unit | PDF extraction output normalization | Fixture PDFs |
| Integration | Scoring pipeline: paper → prompt → API call → assessment persisted | `wiremock` for Claude API |
| Integration | Snowball e2e: seed papers → citation fetch → score → expand → dedup | `wiremock` for OpenAlex + Claude |
| Integration | TUI rendering: each view renders without panic on test backend | `ratatui::backend::TestBackend` |
| Integration | Chat engine: tool dispatch loop with mock tools | Unit test with mock adapters |
| Contract | Claude API response schema stability | Gated live test |
| Contract | Unpaywall API | Gated live test |
| Property | Snowball never revisits a paper (cycle-free invariant) | `proptest` with generated citation graphs |
| Snapshot | Assessment JSON, TUI layout screenshots | `insta` |

**Exit criteria:**
- `scitadel assess --question Q1 --search S1` scores all papers and persists assessments with provenance
- `scitadel snowball --search S1 --depth 2 --threshold 0.7` expands the corpus via citation chaining
- `scitadel tui` launches an interactive dashboard with functional tabs
- Local Ollama scoring works as fallback when no API key is configured
- ≥80% coverage on `scitadel-scoring`, `scitadel-tui`

---

### Phase 3 — Typst Plugin, Collections & Zotero Parity

**Goal:** Close the feature gap with Zotero. User-defined collections, tags,
notes, metadata editing. Typst integration for live bibliography. Import from
Zotero/BibTeX.

**Deliverables:**

| # | Deliverable | Crate | Detail |
|---|---|---|---|
| 3.1 | Collections & tags | `scitadel-core` + `scitadel-db` | User-defined collections (folders), tags on papers, smart collections (saved filters) |
| 3.2 | Notes & annotations | `scitadel-core` + `scitadel-db` | Per-paper user notes (markdown), highlight annotations linked to extractions |
| 3.3 | Metadata editing | `scitadel-core` | Manual correction of title, authors, year, journal, DOI — tracked as user edits |
| 3.4 | Zotero import | `scitadel-export` | Import from Zotero SQLite DB (`zotero.sqlite`) and/or Zotero RDF/JSON export |
| 3.5 | BibTeX import | `scitadel-export` | Parse `.bib` files and ingest as papers |
| 3.6 | Typst bib watcher | `scitadel-cli` | `scitadel bib watch` — watches DB, regenerates `.bib` on change for Typst/LaTeX |
| 3.7 | Typst WASM plugin | `scitadel-typst` | Compile to WASM, read SQLite DB, resolve `@cite-key` to bibliography entries directly in Typst |
| 3.8 | RSS/feed monitoring | `scitadel-adapters` | Monitor journal RSS feeds for new publications matching saved queries |
| 3.9 | DB migrations | `scitadel-db` | `004_collections_tags.sql`, `005_notes_annotations.sql` |

**Tests:**

| Test type | What | How |
|---|---|---|
| Unit | Collection CRUD, tag assignment, smart collection filter evaluation | In-memory SQLite |
| Unit | BibTeX parser correctness | Fixture `.bib` files, snapshot output |
| Unit | Zotero import mapping | Fixture Zotero DB |
| Integration | Bib watcher: modify DB → `.bib` file updates | Temp dir with file watcher |
| Integration | Typst WASM plugin: compile, load in Typst, resolve citations | `typst` CLI compile with plugin |
| Snapshot | Generated `.bib` from DB | `insta` |
| Snapshot | Typst-rendered bibliography page | Compare PDF output |

**Exit criteria:**
- Papers can be organized into collections and tagged
- Notes can be attached to papers
- `scitadel import zotero ~/Zotero/zotero.sqlite` migrates a Zotero library
- `scitadel bib watch` keeps a `.bib` file in sync with the DB
- Typst documents can cite from the DB via WASM plugin or watched `.bib`

---

### Phase 4 — Dolt Sync & Desktop App

**Goal:** Multi-device and multi-user collaboration via Dolt. Tauri-based
desktop app with Zotero-like UX and embedded AI assistant.

**Deliverables:**

| # | Deliverable | Crate | Detail |
|---|---|---|---|
| 4.1 | Dolt repository impl | `scitadel-db` | `sqlx` postgres driver against `dolt sql-server`, same trait impls as SQLite |
| 4.2 | Sync service | `scitadel-sync` | Changeset computation (delta since last sync), bidirectional merge, conflict resolution (LWW for metadata, append-only for events) |
| 4.3 | Sync daemon | `scitadel-sync` | Background `tokio` task, configurable interval, push/pull with progress reporting |
| 4.4 | Sync CLI | `scitadel-cli` | `scitadel sync push`, `scitadel sync pull`, `scitadel sync status`, `scitadel sync config` |
| 4.5 | Tauri app shell | `scitadel-desktop` | Window management, IPC bridge to `scitadel-core`, app state (DB handles) |
| 4.6 | Library browser UI | `scitadel-desktop` | Collections sidebar, paper list, search bar, tag filter |
| 4.7 | Paper detail view | `scitadel-desktop` | Metadata display/edit, abstract, PDF viewer (via `pdfium`), notes editor |
| 4.8 | Search panel | `scitadel-desktop` | Federated search UI with source toggles, live results |
| 4.9 | AI assistant panel | `scitadel-desktop` | Chat panel connected to `scitadel-core` chat engine, tool-augmented |
| 4.10 | Auto-update | `scitadel-desktop` | Tauri updater for self-updating desktop app |

**Tests:**

| Test type | What | How |
|---|---|---|
| Integration | Dolt repository CRUD | `testcontainers` with Dolt Docker image |
| Integration | Sync round-trip: local SQLite → push to Dolt → pull to fresh SQLite → assert equality | `testcontainers` |
| Integration | Conflict resolution: concurrent edits to same paper → deterministic merge | Scripted concurrent writes |
| Integration | Tauri IPC: frontend command → Rust handler → response | Tauri test utilities |
| E2E | Desktop app launches, performs search, displays results | `tauri-driver` (WebDriver) |
| Property | Sync idempotency: push-pull-push produces no diff | `proptest` with generated changesets |

**Exit criteria:**
- `scitadel sync push` sends local changes to a Dolt remote
- `scitadel sync pull` merges remote changes into local SQLite
- Desktop app launches, searches, displays papers, and chats with Claude
- Two users can sync to the same Dolt remote without data loss

---

### Phase 5 — Self-Hosted Web App

**Goal:** Browser-based Scitadel for teams and institutions. Shibboleth/OpenAthens
auth enables institutional full-text access. Claude integration for AI-powered
research assistance.

**Deliverables:**

| # | Deliverable | Crate | Detail |
|---|---|---|---|
| 5.1 | Axum API server | `scitadel-web` | REST API mirroring MCP tools, JWT session management |
| 5.2 | Shibboleth/OpenAthens auth | `scitadel-web` | SAML 2.0 service provider, institutional identity extraction, session cookie |
| 5.3 | Standalone auth | `scitadel-web` | Email/password + TOTP fallback for non-institutional users |
| 5.4 | Publisher proxy | `scitadel-web` | Proxy requests to publisher APIs using institutional credentials (EZproxy-style) |
| 5.5 | Web frontend | `scitadel-web` | SvelteKit or Leptos SPA — library, search, paper view, AI chat |
| 5.6 | Claude integration | `scitadel-web` | WebSocket chat with tool-augmented Claude, configurable: API key pool or user-provided |
| 5.7 | Multi-tenant | `scitadel-web` | Per-user libraries on shared Dolt backend, team sharing via Dolt branches |
| 5.8 | Full-text storage | `scitadel-web` | S3/MinIO for PDFs, content-addressed by hash |
| 5.9 | Deployment | `scitadel-web` | Docker Compose: Axum + Dolt + MinIO + Caddy |
| 5.10 | Admin dashboard | `scitadel-web` | User management, usage stats, API key rotation |

**Tests:**

| Test type | What | How |
|---|---|---|
| Integration | API routes: authenticated requests → correct responses | `axum::test` + mock auth |
| Integration | SAML auth flow: IdP assertion → session creation | Mock SAML IdP |
| Integration | Publisher proxy: institutional request → full-text PDF | `wiremock` publisher |
| E2E | Full web flow: login → search → view paper → chat with AI | Playwright |
| Load | Concurrent users, search throughput | `k6` or `criterion` benchmarks |
| Security | Auth bypass attempts, injection, CSRF | Manual + `cargo-audit` |

**Exit criteria:**
- Web app deployable via `docker compose up`
- Shibboleth login works with a test IdP
- Users can search, browse, annotate, and chat
- Full-text PDFs accessible through institutional proxy
- Multi-user with isolated libraries and optional sharing

---

## 6. Cross-Cutting Concerns

### 6.1 Database Migration Strategy

```
migrations/
├── 001_initial.sql              # papers, searches, search_results, questions, terms, assessments
├── 002_citations.sql            # citations, snowball_runs
├── 003_full_text.sql            # extractions
├── 004_collections_tags.sql     # collections, tags, paper_collections, paper_tags
├── 005_notes_annotations.sql    # notes, annotations
├── 006_sync_metadata.sql        # sync_log, last_sync_at, change_vectors
```

Migrations are plain SQL files, applied by `scitadel-db` at startup. Both
SQLite and Dolt implementations use the same migration files (dialect
differences handled by conditional blocks if needed).

### 6.2 Configuration Hierarchy

```
1. CLI flags              (highest priority)
2. Environment variables  (SCITADEL_*)
3. Workspace config       (.scitadel/config.toml)
4. User config            (~/.config/scitadel/config.toml)
5. Compiled defaults      (lowest priority)
```

```toml
# .scitadel/config.toml
[db]
path = ".scitadel/scitadel.db"

[sync]
remote = "https://dolt.example.com/scitadel"
interval = "5m"

[scoring]
model = "claude-sonnet-4-20250514"
temperature = 0.2
max_concurrent = 5

[sources.pubmed]
api_key_env = "NCBI_API_KEY"
timeout = "30s"

[sources.openalex]
email = "researcher@example.edu"
```

### 6.3 Secret Management

- API keys are **never** stored in config files
- Keys are read from environment variables or system keyring (`keyring` crate)
- CLI: `scitadel secrets set ANTHROPIC_API_KEY`
- Config references keys by env var name, not value

### 6.4 Binary Distribution

| Channel | Method |
|---|---|
| Source | `cargo install scitadel` |
| Homebrew | Tap with prebuilt bottles |
| Nix | Flake with `scitadel` package |
| GitHub Releases | Cross-compiled binaries (Linux x86_64/aarch64, macOS x86_64/aarch64, Windows x86_64) |
| Docker | `ghcr.io/vig-os/scitadel` for server deployment |
| Typst | Published to Typst package registry |

### 6.5 Versioning & Release Strategy

- Workspace-level version in root `Cargo.toml`
- All crates share the same version (lockstep)
- SemVer: breaking trait changes = major bump
- `cargo-release` for automated version bumping, tagging, publishing
- Changelog via `git-cliff`

### 6.6 CI Pipeline

```yaml
# .github/workflows/ci.yml
on: [push, pull_request]

jobs:
  lint:
    - cargo fmt --check
    - cargo clippy -- -D warnings
    - cargo deny check
    - cargo machete
    - taplo check

  test:
    matrix: [ubuntu-latest, macos-latest, windows-latest]
    - cargo nextest run
    - cargo llvm-cov --lcov > coverage.lcov

  contract-tests:  # weekly, or manual trigger
    - cargo nextest run --features contract-tests

  build:
    - cargo build --release
    - Upload artifacts
```

---

## 7. Feature Matrix

What lands when, across all product surfaces:

| Feature | P0 | P1 | P2 | P3 | P4 | P5 |
|---|---|---|---|---|---|---|
| Domain models & traits | x | | | | | |
| SQLite persistence | x | | | | | |
| PubMed adapter | | x | | | | |
| arXiv adapter | | x | | | | |
| OpenAlex adapter | | x | | | | |
| INSPIRE-HEP adapter | | x | | | | |
| Federated search | | x | | | | |
| Dedup & canonicalization | | x | | | | |
| BibTeX/JSON/CSV export | | x | | | | |
| CLI | | x | | | | |
| MCP server | | x | | | | |
| Research questions & terms | | x | | | | |
| Search history & diff | | x | | | | |
| Claude API scoring | | | x | | | |
| Ollama local scoring | | | x | | | |
| Provenance tracking | | | x | | | |
| Citation snowballing | | | x | | | |
| Full-text retrieval (OA) | | | x | | | |
| Crossref adapter | | | x | | | |
| Semantic Scholar adapter | | | x | | | |
| TUI dashboard | | | x | | | |
| Chat engine | | | x | | | |
| Collections & tags | | | | x | | |
| Notes & annotations | | | | x | | |
| Metadata editing | | | | x | | |
| Zotero import | | | | x | | |
| BibTeX import | | | | x | | |
| Typst bib watcher | | | | x | | |
| Typst WASM plugin | | | | x | | |
| RSS feed monitoring | | | | x | | |
| Dolt repository impl | | | | | x | |
| SQLite ↔ Dolt sync | | | | | x | |
| Tauri desktop app | | | | | x | |
| AI assistant (desktop) | | | | | x | |
| Axum web server | | | | | | x |
| Shibboleth/OpenAthens | | | | | | x |
| Publisher proxy | | | | | | x |
| Web frontend | | | | | | x |
| Multi-tenant | | | | | | x |

---

## 8. Risk Register

| # | Risk | Severity | Phase | Mitigation |
|---|---|---|---|---|
| R1 | Rust rewrite takes longer than Python iteration | High | 0–1 | Python prototype already validated the design; port a known architecture, don't redesign |
| R2 | rmcp immaturity — breaking changes | Medium | 1 | Pin version, wrap in thin abstraction, contribute upstream |
| R3 | Dolt PostgreSQL wire protocol gaps | Medium | 4 | SQLite is always the primary; Dolt is additive. Test early with `testcontainers` |
| R4 | Typst WASM plugin API instability | Low | 3 | Bib watcher is the fallback — works without any Typst integration |
| R5 | Tauri v2 complexity for rich desktop app | Medium | 4 | Start with simple IPC, grow incrementally. TUI remains the power-user interface |
| R6 | SAML/Shibboleth integration complexity | Medium | 5 | Use `mod_auth_openidc` as reverse proxy; don't implement SAML in Rust |
| R7 | API rate limiting at scale (web app) | Medium | 5 | Per-user quotas, request queuing, caching layer |
| R8 | Solo developer bus factor | High | all | Open-source early, document decisions, keep architecture simple |
| R9 | Publisher legal concerns for full-text proxy | Medium | 5 | Only proxy for institutions with active licenses; respect publisher ToS |
| R10 | SQLite ↔ Dolt schema drift | Medium | 4 | Single source of truth for migrations; both implementations tested against same SQL |

---

## 9. Open Questions

| # | Question | Decision needed by | Options |
|---|---|---|---|
| Q1 | Desktop frontend framework: Svelte or Leptos? | Phase 4 start | Svelte: mature ecosystem, more contributors. Leptos: full Rust stack, WASM. |
| Q2 | Web frontend: same as desktop or separate? | Phase 5 start | Shared component library vs separate UIs. Shared saves effort but couples. |
| Q3 | Dolt hosting: DoltHub managed or self-hosted? | Phase 4 start | DoltHub: zero ops. Self-hosted: full control, no vendor dependency. |
| Q4 | PDF viewer in desktop: `pdfium` or webview-native? | Phase 4 | `pdfium` via FFI: fast, complex. Webview `<embed>` or `pdf.js`: simpler. |
| Q5 | Local LLM: Ollama only or also llama.cpp direct? | Phase 2 | Ollama: simpler (HTTP API). llama.cpp: no external process, but heavy FFI. |
| Q6 | Typst plugin: direct SQLite read or IPC to running scitadel? | Phase 3 | Direct read: standalone, but locks DB. IPC: needs running process. |
| Q7 | Should `scitadel-core` expose a C FFI for potential Python/Swift bindings? | Phase 4+ | Enables non-Rust frontends, but adds maintenance burden. |

---

## 10. Success Metrics

| Metric | Target | Phase |
|---|---|---|
| `cargo install scitadel` works on Linux/macOS/Windows | Yes | 1 |
| Search across 4 sources returns deduplicated results | Yes | 1 |
| MCP server passes all tool invocations | Yes | 1 |
| LLM scoring recall vs human gold standard | ≥90% | 2 |
| Snowball discovers relevant papers missed by direct search | ≥3 per run | 2 |
| TUI renders all views without panic | Yes | 2 |
| Zotero library import preserves ≥95% of metadata | Yes | 3 |
| Typst document compiles with DB-sourced citations | Yes | 3 |
| Sync round-trip (push + pull) preserves 100% of data | Yes | 4 |
| Desktop app cold start | <2s | 4 |
| Web app handles 50 concurrent users | Yes | 5 |
| Shibboleth SSO works with ≥2 test IdPs | Yes | 5 |

---

## 11. Decision

Approved to proceed. Begin with **Phase 0: scaffold the Rust workspace** and
port domain models from the Python prototype.
