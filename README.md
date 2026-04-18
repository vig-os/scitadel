# Scitadel

Programmable, reproducible scientific literature retrieval.

Scitadel is a CLI and TUI tool that runs federated searches across multiple academic databases, deduplicates results, scores relevance with LLMs, chains citations via snowballing, and exports in structured formats. Every search is persisted and reproducible.

## Why

Scientific literature retrieval is fragmented, manual, and non-reproducible. Researchers must search PubMed, arXiv, INSPIRE-HEP, and OpenAlex independently — each with different query syntax, metadata schemas, and export formats. There is no unified, scriptable interface.

Scitadel fixes this: one query, all sources, deterministic results, full audit trail.

## Install

Scitadel is a Rust workspace. You need a Rust toolchain (`rustup`, stable channel).

### From source (recommended today)

```bash
git clone https://github.com/vig-os/scitadel.git
cd scitadel
cargo install --path crates/scitadel-cli --locked
```

This drops a single `scitadel` binary into `~/.cargo/bin` (make sure that's on your `PATH`). CLI, TUI, and MCP server are all subcommands of the same binary.

### As a Claude MCP server

**User scope (available in every session, everywhere):**

```bash
claude mcp add --scope user scitadel -- scitadel mcp
```

**Project scope (committed to the repo, available when cwd is this project):**

The repo ships a `.mcp.json` that registers the `scitadel` binary from `PATH`. Just run `cargo install --path crates/scitadel-cli` once and Claude Code will pick it up automatically.

**Local/session scope (no commit, just this machine):**

```bash
claude mcp add --scope local scitadel -- scitadel mcp
```

Verify with `claude mcp list`.

## Quick start

```bash
# Initialize the database (creates ./.scitadel/scitadel.db)
scitadel init

# Store credentials in your OS keychain (one-time, per source)
scitadel auth login pubmed
scitadel auth login openalex
scitadel auth status

# Run a federated search
scitadel search "PET tracer development" -s pubmed,arxiv,openalex,inspire -n 20

# View past searches / show a paper / export
scitadel history
scitadel show <paper-or-search-id>
scitadel export <search-id> --format bibtex --output results.bib

# Download a paper by DOI (OA PDF via Unpaywall, else publisher HTML)
scitadel download 10.1038/s41586-020-2649-2

# Launch the interactive TUI
scitadel tui
```

## Usage patterns

### 1. Search, review, export

The core loop: one query hits all sources, results are deduplicated and persisted.

```bash
# Search across all four sources
scitadel search "CRISPR Cas9 gene therapy" -n 30

# Check what came back
scitadel history

# Export to BibTeX for your paper
scitadel export a3f --format bibtex -o references.bib

# Or get structured JSON for downstream processing
scitadel export a3f --format json -o results.json
```

Search IDs support prefix matching — `a3f` will match if unambiguous.

Re-running the same query creates a new search record. Both runs are persisted and can be compared to see what changed as the underlying corpus evolves.

### 2. Question-driven search and scoring

Link search terms to a research question, then let scitadel build the query automatically:

```bash
# 1. Define what you're looking for
scitadel question create "What PET tracers are used in oncology?" \
  -d "Focus on clinical applications post-2020"

# 2. Add search term groups
scitadel question add-terms <question-id> "PET" "tracer" "oncology"
scitadel question add-terms <question-id> "positron emission" "radiotracer" \
  --query "positron emission tomography AND radiotracer"

# 3. Search using linked terms (auto-builds query with OR)
scitadel search -q <question-id>

# Or combine with an explicit query
scitadel search "FDG clinical trial" -q <question-id>

# 4. Score every paper against your question
export ANTHROPIC_API_KEY=sk-ant-...
scitadel assess <search-id> -q <question-id>

# 5. Export the scored results
scitadel export <search-id> --format json -o scored_results.json
```

The `assess` command sends each paper's title, authors, year, and abstract to Claude with a structured scoring rubric (0.0-1.0). Each assessment stores the full provenance: score, reasoning, exact prompt, model name, and temperature — so results are auditable and re-runnable with different models.

```bash
# Tune scoring parameters
scitadel assess <search-id> -q <question-id> \
  --model claude-sonnet-4-6 --temperature 0.2
```

### 3. Citation chaining (snowballing)

Expand your corpus by following citation chains from discovered papers. Scitadel fetches references (backward) and citing papers (forward) from OpenAlex, scores each against your research question, and only follows leads that pass a relevance threshold.

```bash
# Snowball from a search's papers
scitadel snowball <search-id> -q <question-id>

# Control depth, direction, and threshold
scitadel snowball <search-id> -q <question-id> \
  --depth 2 \
  --direction both \
  --threshold 0.7 \
  --model claude-sonnet-4-6
```

Options:
- `--depth` — how many levels to chase (1-3, default 1)
- `--direction` — `references` (backward), `cited_by` (forward), or `both` (default)
- `--threshold` — minimum relevance score to continue expanding (default 0.6)

New papers discovered via snowballing are deduplicated against the existing database, and all citation edges are persisted for later exploration.

### 4. Interactive TUI

Browse searches, papers, assessments, and citation trees in an interactive terminal dashboard.

```bash
# Launch the TUI (requires textual: pip install scitadel[tui])
scitadel tui

# Or use the standalone entry point
scitadel-tui
```

The TUI has three tabs:

- **Searches** — browse past search runs, drill into papers, view full metadata and assessments
- **Questions** — see research questions with their linked terms and assessment statistics
- **New Search** — run a search and watch results stream in

From any paper, press `c` to view its citation tree (references and citing papers from snowball runs).

### 5. Agent-driven workflow via MCP

Connect scitadel as an MCP server and let an LLM agent drive the pipeline — from question formulation through search, scoring, and snowballing.

```bash
# Run the server manually (stdio transport)
scitadel mcp
```

For Claude Code, register once (see [Install](#install) above):

```bash
claude mcp add --scope user scitadel -- scitadel mcp
```

For Claude Desktop, add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "scitadel": {
      "command": "scitadel",
      "args": ["mcp"]
    }
  }
}
```

The MCP server exposes 14 tools:

| Tool | What it does |
|------|-------------|
| `search` | Federated search (supports `question_id` for auto-query) |
| `list_searches` | Browse past search runs |
| `get_papers` | List papers from a search |
| `get_paper` | Full details of a single paper |
| `export_search` | Export as BibTeX / JSON / CSV |
| `create_question` | Define a research question |
| `list_questions` | Browse research questions |
| `add_search_terms` | Link keywords to a question |
| `assess_paper` | Record a relevance score with reasoning |
| `get_assessments` | Retrieve scores for a paper or question |
| `prepare_assessment` | Build a rubric + paper payload for host-LLM scoring |
| `prepare_batch_assessments` | Same, for every paper in a search |
| `save_assessment` | Persist a host-LLM-scored assessment |
| `download_paper` | Fetch PDF (Unpaywall) or publisher HTML by DOI |

This enables a workflow where the agent formulates a question, generates search terms, runs a search, scores each paper, snowballs relevant citations, and writes structured assessments — all through tool calls with no manual intervention.

## Workflow coverage

Where scitadel stands against the [full envisioned pipeline](docs/rfcs/RFC-001-2026-02-26-scitadel-problem-space.md):

| Step | Status | Detail |
|------|--------|--------|
| Research question formulation | **Done** | CLI + MCP |
| Search term management | **Done** | CLI `question add-terms` + MCP, `search --question` auto-builds queries |
| Federated search (4 sources) | **Done** | PubMed, arXiv, OpenAlex, INSPIRE-HEP in parallel |
| Deduplication and merge | **Done** | DOI exact + title similarity, cross-source metadata fill |
| LLM relevance scoring | **Done** | CLI `assess` + MCP `assess_paper`, full provenance |
| Citation chaining | **Done** | Forward/backward snowballing via OpenAlex, relevance-gated |
| Structured export | **Done** | BibTeX, JSON, CSV |
| Reproducible audit trail | **Done** | Every search, assessment, citation edge, and scoring prompt persisted |
| Interactive TUI | **Done** | Textual dashboard with search/paper/assessment/citation browsing |
| Full-text retrieval | Planned | OA papers via Unpaywall, PDF extraction |
| Chat with papers | Planned | RAG over extracted text |
| Knowledge graph | Planned | Citation network visualization |

## Sources

| Source | API | Notes |
|--------|-----|-------|
| **PubMed** | E-utilities (esearch + efetch) | Set `SCITADEL_PUBMED_API_KEY` for higher rate limits |
| **arXiv** | Atom feed | No key required |
| **OpenAlex** | REST via PyAlex | Set `SCITADEL_OPENALEX_EMAIL` for polite pool |
| **INSPIRE-HEP** | REST API | No key required |

## CLI reference

```
scitadel search [QUERY]                Run a federated search (query optional with -q)
scitadel history                       Show past search runs
scitadel export <id>                   Export results (bibtex, json, csv)
scitadel question create <text>        Create a research question
scitadel question list                 List research questions
scitadel question add-terms <qid> ...  Link search terms to a question
scitadel assess <search-id>            Score papers against a question with Claude
scitadel snowball <search-id>          Run citation chaining from a search
scitadel tui                           Launch the interactive TUI
scitadel mcp                           Start the MCP server (stdio)
scitadel download <doi>                Fetch PDF (Unpaywall) or publisher HTML
scitadel auth login <source>           Store credentials in OS keychain
scitadel auth status                   List configured credentials
scitadel init                          Initialize the database
```

### Search options

```
-q, --question     Research question ID (auto-builds query from linked terms)
-s, --sources      Comma-separated sources (default: pubmed,arxiv,openalex,inspire)
-n, --max-results  Maximum results per source (default: 50)
```

### Export options

```
-f, --format   Output format: bibtex, json, csv (default: json)
-o, --output   Write to file instead of stdout
```

### Assess options

```
-q, --question      Research question ID (required)
-m, --model         Model for scoring (default: claude-sonnet-4-6)
-t, --temperature   Temperature for scoring (default: 0.0)
```

### Snowball options

```
-q, --question      Research question ID (required)
-d, --depth         Max chaining depth, 1-3 (default: 1)
--direction         references, cited_by, or both (default: both)
--threshold         Min relevance score to expand (default: 0.6)
-m, --model         Model for scoring (default: claude-sonnet-4-6)
```

### Question add-terms options

```
-q, --query   Custom query string (default: terms joined by spaces)
```

## Configuration

Credentials resolve in this order: **OS keychain → environment variable → `.scitadel/config.toml` → empty**. For most users the keychain path is best — `scitadel auth login <source>` prompts you and stores the secret securely.

| Source | Keychain key | Env var | Notes |
|--------|-------------|---------|-------|
| PubMed | `pubmed.api_key` | `SCITADEL_PUBMED_API_KEY` | Optional, higher rate limits |
| OpenAlex | `openalex.email` | `SCITADEL_OPENALEX_EMAIL` | Polite pool |
| PatentsView | `patentsview.api_key` | `SCITADEL_PATENTSVIEW_KEY` | Free registration |
| Lens | `lens.api_token` | `SCITADEL_LENS_TOKEN` | Free tier |
| EPO OPS | `epo.consumer_key` + `epo.consumer_secret` | `SCITADEL_EPO_KEY`, `SCITADEL_EPO_SECRET` | Registered app |
| Anthropic | _(not stored)_ | `ANTHROPIC_API_KEY` | Required for `assess`, `snowball`, MCP scoring |

Other knobs:

| Variable | Default | Description |
|----------|---------|-------------|
| `SCITADEL_DB` | `./.scitadel/scitadel.db` | Database path |
| `SCITADEL_CHAT_MODEL` | `claude-sonnet-4-6` | Model used for scoring |
| `SCITADEL_CHAT_MAX_TOKENS` | `4096` | Max completion tokens |
| `SCITADEL_SCORING_CONCURRENCY` | `5` | Parallel scoring requests |

## Architecture

Hexagonal (ports and adapters), implemented as a Rust workspace:

```
scitadel-cli (clap) / scitadel-mcp (rmcp) / scitadel-tui (ratatui)
  -> scitadel-core (services, domain, ports)
    -> scitadel-db        (rusqlite adapters)
    -> scitadel-adapters  (PubMed, arXiv, OpenAlex, INSPIRE-HEP,
                           PatentsView, Lens, EPO OPS, Unpaywall)
    -> scitadel-scoring   (Anthropic SDK)
    -> scitadel-export    (BibTeX, JSON, CSV)
```

- **Domain models** define `Paper`, `Search`, `ResearchQuestion`, `Assessment`, `Citation`, `SnowballRun`
- **Repository ports** are traits — SQLite today, swap for Postgres without touching services
- **Source adapters** run in parallel with retry/backoff; partial failures don't abort the search
- **Dedup engine** validates and normalizes DOIs, merges by DOI (exact) then title similarity (Jaccard), filling metadata gaps across sources
- **Snowball service** chains citations with relevance-gated traversal, depth limiting, and deduplication

## Data model

A **paper** exists once, regardless of how many searches found it. A paper's relevance is not intrinsic — it's relative to a **research question**, captured as an **assessment** with score, reasoning, and provenance (human vs. model). **Citations** are directed edges between papers, discovered via snowball runs.

```
ResearchQuestion -< SearchTerm
                 -< Assessment >- Paper
                 -< SnowballRun
Search -< SearchResult >- Paper
Search -< SourceOutcome
Paper -< Citation >- Paper
```

## Development

Requires a stable Rust toolchain.

```bash
# Build
cargo build

# Run the binary without installing
cargo run -- search "PET tracer"

# Run tests (workspace-wide)
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

## Prebuilt binaries

Every tagged release attaches tarballs for Linux x86_64, macOS x86_64, and
macOS arm64 to the [Releases page](https://github.com/vig-os/scitadel/releases).
Download the one for your platform, extract, and put the `scitadel` binary on
your `$PATH`:

```sh
# Example for macOS arm64 (Apple Silicon)
curl -L https://github.com/vig-os/scitadel/releases/latest/download/scitadel-X.Y.Z-aarch64-apple-darwin.tar.gz \
  | tar xz
sudo mv scitadel-X.Y.Z-aarch64-apple-darwin/scitadel /usr/local/bin/
scitadel --version
```

Each release also ships a `.sha256` file next to each tarball — verify with
`shasum -a 256 -c <file>.sha256`.

## License

Dual-licensed under either of

- [Apache License 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
