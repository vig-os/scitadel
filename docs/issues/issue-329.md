---
type: issue
state: open
created: 2026-11-09T03:21:59Z
updated: 2026-11-09T03:21:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/329
comments: 0
labels: feature, effort:medium, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: 266, 307, 308
synced: 2026-11-09T08:50:52.710Z
---

# [Issue 329]: [feat(mcp): H17 — standalone scitadel-mcp with grounded, licence-tagged responses](https://github.com/vig-os/scitadel/issues/329)

Part of #311. Design: ADR-008 (PR #317), positioning (Context), §7. Lives in scitadel.

## Motivation
The hub is a stepping stone toward an MCP that grounds AI agents: answers carry citations, provenance and a licence class, so an agent can't silently fabricate a reference. Agent builders are the first users, so `scitadel-mcp` must install alone and work without a hub.

## Scope
- Distribute `scitadel-mcp` standalone: crates.io, a PyPI wheel, npm and Homebrew, with a local SQLite cache and no hub dependency.
- Versioned tool schemas (`tool_version`), stable across releases, with a deprecation policy.
- Grounded responses: every record carries its identifiers, a citation, provenance (source, extractor, version) and a per-row licence class.
- With a hub configured, the same tools are served from the hub, transparently.

## Acceptance
- `pipx install` / `npx` / `brew install` each yield a working MCP server against live APIs.
- Snapshot tests pin the tool schemas.
- A response without a citation or licence class fails a test.

## Depends on
H3 (#316).

