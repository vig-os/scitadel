# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- **TUI two-pane annotation reader** (#97, iter 1). Toggled with `R`
  on the paper detail overlay: left pane renders the paper body
  (`full_text` if cached, else the abstract) with background-color
  highlights over annotated ranges, palette hashed by thread root_id
  so a thread keeps the same color on every render. Right pane
  threads root annotations + replies, focused row syncs with the
  highlight underline. `J` / `K` hop between highlights. Single-
  pane fallback message when no body text is available yet.
  Multi-line gutter bars and PDF overlay are deferred.
- **`PaperRepository::update_full_text`** + caching in
  `read_paper_tool` (#97 prep). The MCP read tool now persists the
  extracted text on first call so subsequent reads — including the
  new TUI reader — hit the DB instead of re-running pdf-extract.
  Failure to cache is non-fatal (extraction still returns).
- **VHS coverage for CLI search + question subcommands** (#99): two
  new tapes (`tests/vhs/cli-search.tape`,
  `tests/vhs/cli-question-workflow.tape`) plus a `Shift+Tab`
  (BackTab) + `d`/`u` page-scroll addition to `tui-launch.tape` to
  close the gaps flagged in the 0.4.0 coverage audit.

### Changed

- **MCP annotation tool descriptions** (#100): `create_annotation`,
  `reply_annotation`, `update_annotation`, `delete_annotation`, and
  `list_unread` now flag trust-on-first-use author identity (real auth
  ships with the Phase-5 Dolt sync layer) and the wall-clock-based
  read-receipt race window (`seen_at < updated_at`). Every annotation
  write now emits a `tracing::info!` audit record (op + ids + author).

### Removed

### Fixed

### Security

## [0.3.0] - 2026-04-19

Agent DX polish + annotations. 14 PRs merged to `dev` under a
loop-driven autonomous execution session (ADR-003). Everything behind
the VHS coverage gate and the existing Rust CI bar.

Note on scope: #58 (MCP progress notifications) was deferred to 0.4.0
after discovering rmcp 0.1.5's tool macro doesn't inject a `Peer`
reference into handlers — see ADR-003 for the rationale.

### Added

- **MCP `list_sources`** (#54): per-source metadata (name, description,
  required credential fields, configured-in-this-env flag, rate-limit
  hint) so agents can introspect instead of guessing.
- **MCP `summarize_search`** (#53): one-call JSON digest of every
  paper in a search with truncated abstracts, saves N round-trips.
- **MCP `get_rubric`** (#56): cacheable access to the static scoring
  rubric so agents don't pay for it per-paper via
  `prepare_assessment`.
- **MCP `search` returns structured JSON** (#55): per-source
  `status / result_count / latency_ms / error` alongside a
  `summary` string for back-compat with human readers.
- **MCP `find_similar_searches`** (#57): FTS5 (porter + unicode61)
  over stored query strings; backed by new migration 006 with a
  trigger-based sync. FTS5-operator sanitizer so arbitrary user
  input doesn't raise syntax errors.
- **Annotations** (#49) — shipped across 5 iterations:
  - **iter 1 — data model + schema**: `annotations` +
    `annotation_reads` tables (migration 005), W3C-style multi-
    selector anchor (position, quote + context, sentence-id),
    threaded replies, soft-delete tombstones.
  - **iter 2 — repo + anchoring resolver**:
    `SqliteAnnotationRepository` with CRUD + thread loading;
    `resolve_anchor` tries position → quote-substring → orphan
    (fuzzy + sentence-id deferred to 3b).
  - **iter 3 — TUI view-only rendering**: annotations listed in
    the paper detail overlay; threaded replies indented.
  - **iter 4 — MCP CRUD**: `create_annotation`, `reply_annotation`,
    `update_annotation`, `delete_annotation`, `list_annotations`.
    Author identity is mandatory on writes.
  - **iter 5 — read receipts**: `mark_seen`, `mark_thread_seen`,
    `list_unread`. Edits auto-resurface rows as unread.
- **ADR-003** — 0.3.0 execution tracker + decision log.

### Changed

- **`search` tool response shape** is now JSON (still carries a
  `summary` string for back-compat). External clients that parsed
  the old string should read `summary` or pivot to the structured
  fields.
- `get_papers_tool` + `prepare_batch_assessments_tool` now use
  `truncate_abstract` (char-safe) instead of the byte-slice
  `&s[..300]` that panicked on non-ASCII content.
- `list_sources` OpenAlex credential field renamed from `email` to
  `polite_pool_email` to match how config actually stores it.

### Removed

- Python-era release workflows (`release.yml`, `release-core.yml`,
  `release-publish.yml`, `release-extension.yml`,
  `prepare-release.yml`, `promote-release.yml`) — all superseded by
  the Rust-native `binaries.yml` + `publish-crates.yml` shipped in
  0.2.0.

### Deferred to later milestones

- **MCP progress notifications (#58)** — 0.4.0. Waits for rmcp
  upgrade or custom tool-handler wrapper.
- **TUI-native annotation create/edit/delete (#92)** — 0.4.0.
  MCP CRUD covers it for now.
- **Fuzzy anchor matching + sentence-id resolver** — follow-up
  once the TUI surfaces orphans.
- **`get_annotated_paper` composite endpoint** — waits for the
  two-pane reader design.

## [0.2.0] - 2026-04-18

Onboarding and reading workflow. Eight PRs, every UX/TUI change backed
by a VHS tape, CI gate prevents future UI work from skipping tapes.

Note on original scope: #49 annotations is deferred to 0.3.0 — it is
an architectural effort (multi-selector anchoring, threaded replies,
two-pane reader, full MCP CRUD) that benefits from its own focused
release rather than being rushed alongside the onboarding work.

### Added

- **`scitadel init` wizard** (#47): interactive first-run setup with
  prompts for email + sources, non-interactive `--yes` mode, writes
  `config.toml`, migrates the DB, prints a ready-to-run sample query.
- **Star papers in the TUI** (#48, v1): `s` toggles a per-reader ★ flag
  on the Papers tab. New `paper_state` table scoped by reader; the
  schema already has `to_read` / `read_at` columns for the upcoming
  Queue tab in a follow-up.
- **Institutional-access hint on paywalled downloads** (#50): when
  `AccessStatus::Paywall` is detected, the task panel shows the live
  publisher URL + a note that an institutional IP range may grant
  access. Gated by `UiConfig.show_institutional_hint` (default on).
- **OFFLINE indicator** (#51, v1): startup network probe; yellow bold
  `[OFFLINE]` in the status bar when the probe fails. Reads continue
  to work from local SQLite. `SCITADEL_FORCE_OFFLINE=1` env var
  bypasses the probe for testing.
- **Prebuilt binaries** (#64): new `binaries.yml` workflow builds
  scitadel-cli on linux-x64 + macos-x64 + macos-arm64 on every semver
  tag push, tarballs + SHA256 sums attached to the GitHub Release.
- **Crates.io publishing pipeline** (#65): `publish-crates.yml`
  dry-runs metadata on every PR, sequential live-publish on tag
  push in dependency order (core → db → adapters → scoring → export
  → mcp → tui → cli). Requires `CARGO_REGISTRY_TOKEN` secret.
- **VHS walkthrough-tape harness** (#62): `tests/vhs/` + `just vhs`
  recipes + CI workflow that installs vhs on ubuntu, runs every tape
  on PR/push, uploads snapshots as artifacts. Coverage gate: PRs that
  touch TUI/CLI source must add or update a tape (or include
  `[tape-exempt: <reason>]` in a commit).

### Changed

- Every merged UX/TUI PR in this release ships with a VHS tape. Any
  future PR that misses the gate will be blocked by CI.

### Deferred

- **Annotations** (#49) — threaded notes, read receipts, multi-selector
  anchoring, two-pane reader. Moved to 0.3.0.
- **Reading queue: `r`/`R` keybindings + Queue tab** — data model exists;
  UI is a follow-up.
- **Offline retry queue** — the indicator ships; queued retries don't.

## [0.1.0] - 2026-04-18

Initial release. Rust workspace implementing the scitadel MVP: federated
scientific literature search with structured storage and retrieval/assessment
tooling across CLI, MCP server, and TUI surfaces.

### Added

- **Core domain** (`scitadel-core`): paper / search / question / assessment
  models, keychain + env-var credential resolution, strict DOI validation and
  canonicalization.
- **SQLite persistence** (`scitadel-db`): schema + migrations, deduplication on
  DOI / OpenAlex / arxiv identifiers, upsert with conflict resolution (#40).
- **Source adapters** (`scitadel-adapters`): PubMed, arXiv, OpenAlex,
  INSPIRE-HEP, EPO OPS, PatentsView, Lens. Paper download chain (arxiv →
  OpenAlex → Unpaywall → publisher HTML) with paywall detection.
- **Scoring** (`scitadel-scoring`): Claude API scorer for automated relevance
  assessment.
- **Export** (`scitadel-export`): BibTeX, JSON, CSV.
- **CLI** (`scitadel-cli`): `search`, `history`, `export`, `download`, `score`,
  `auth`, `tui`, `mcp` subcommands.
- **MCP server** (`scitadel-mcp`): 14 tools for agent-driven literature
  workflows, including `search`, `get_papers`, `assess_paper`,
  `prepare_assessment`, `download_paper`, `read_paper`.
- **TUI** (`scitadel-tui`): ratatui-based browser with Searches / Papers /
  Questions tabs, async download task panel, paper reader, vim-ish keybindings.
- **Nix flake + direnv** reproducible devshell.
- **vig-os/devcontainer 0.3.3** workflow/release standards.
- **Dual-license**: MIT OR Apache-2.0.

### Known limitations

- No citation graph / snowball (planned for 0.4.0, see #59).
- No in-TUI annotations (planned for 0.2.0, see #49).
- No prebuilt binaries; install via `cargo install --path crates/scitadel-cli`
  (cargo-dist tracked in #64).
- Not yet published to crates.io (tracked in #65).
