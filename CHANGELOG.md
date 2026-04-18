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
