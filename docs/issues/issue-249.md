---
type: issue
state: closed
created: 2026-09-30T00:11:51Z
updated: 2026-10-02T23:19:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/249
comments: 1
labels: bug, effort:small, security, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:47.369Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on October 2, 2026 at 11:19 PM_

Fixed by #265 (merged into `dev`).

`output_dir` is gone from the MCP request, so it is gone from the **JSON schema an LLM actually reads** — tightening only the Rust signature would have left the tool description implying the capability existed. Downloads always resolve to the configured `papers_dir`; the filename was already derived from the paper's own identifiers, and neither `doi_to_filename` nor `sanitize_filename` can emit a path separator.

The human CLI keeps `scitadel download --output-dir`.

A caller that still sends `output_dir` (older agent, cached description, or an injection) has it **ignored rather than obeyed**. Both tests were confirmed to fail when the field is reintroduced.

Scanning every MCP request for the same shape turned up two more caller-controlled paths — `ImportBibRequest.path` and the bib export output path. Those are the same class but are load-bearing for agent workflows, so they are tracked separately as #266 rather than changed under this issue.

