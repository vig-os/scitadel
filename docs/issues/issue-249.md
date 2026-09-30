---
type: issue
state: open
created: 2026-09-30T00:11:51Z
updated: 2026-09-30T00:11:51Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/249
comments: 0
labels: bug, effort:small, security, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:22.820Z
---

# [Issue 249]: [security(mcp): download_paper accepts an agent-supplied output_dir — arbitrary write location](https://github.com/vig-os/scitadel/issues/249)

## Problem
The MCP `download_paper` request has an `output_dir` field (`crates/scitadel-mcp/src/server.rs:316`). `download_paper_tool` (`crates/scitadel-mcp/src/tools.rs:590`) writes the downloaded file there verbatim. Any agent driving the MCP server, including a prompt-injected one, can therefore make scitadel write attacker-influenced content (publisher HTML, or a PDF from any URL a DOI resolves to) into any directory the user can write to: `~/.config/…/autostart`, a repo's `.githooks/`, shell rc directories and so on.

## Fix
- Remove `output_dir` from the MCP request. Agents always write under the configured `papers_dir`. The CLI can keep a human-supplied `--output-dir`.
- Everything that writes a file generates the filename itself; no caller-controlled path components.
- Test: an MCP call carrying an `output_dir` either fails schema validation or is ignored, and the file lands under `papers_dir`.

Found in the #247 security review. #247's new acquisition verbs must not inherit this shape.

Refs: #247
