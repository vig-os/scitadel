# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

### Changed

### Removed

### Fixed

### Security

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
