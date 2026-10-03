---
type: issue
state: open
created: 2026-10-02T15:41:29Z
updated: 2026-10-02T15:41:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/266
comments: 0
labels: bug, effort:small, security, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:42.296Z
---

# [Issue 266]: [security(mcp): bib import/export accept agent-supplied filesystem paths — same class as #249](https://github.com/vig-os/scitadel/issues/266)

## Problem

Found while fixing #249. The same caller-controlled-path shape that #249 removed from `download_paper` is still present on two other MCP tools.

Located by scanning every `*Request` struct in `crates/scitadel-mcp/src/server.rs` for path-shaped fields:

| Struct | Field | Direction | Line |
|---|---|---|---|
| `ImportBibRequest` | `path` | agent-supplied file **read** | `server.rs:353` |
| bib export request | output path | agent-supplied file **write** | `server.rs:379` |
| `DiffRequest` | `file_a`, `file_b` | agent-supplied file **read** | `server.rs:394,397` |

The export one is the same arbitrary-write as #249: an agent — or a prompt injection driving one — chooses the destination for content scitadel generates, so it can be aimed at an autostart directory, a `.githooks/` path, or an rc file.

The two read paths are a different, milder shape: an agent can make scitadel read and return the contents of an arbitrary file the user can read. Depending on how the result is echoed back, that is an exfiltration primitive rather than an integrity problem, so it needs its own analysis rather than a copy of the #249 fix.

## Why this was not folded into #249

#249's fix principle is "everything that writes a file generates the filename itself; no caller-controlled path components." That applies cleanly to `download_paper` and to the export output path. It does **not** cleanly apply to the read paths, where the file *is* the input the caller is naming — removing `path` from `ImportBibRequest` would remove the tool's entire purpose. Those need a scoping decision (allow only under the workspace? only under a configured bib dir?) that is a product call, not a mechanical fix.

## Proposal

1. **Export output path** — apply the #249 fix directly. scitadel generates the filename under a configured export directory; if the agent wants a specific name it passes a *filename*, never a directory or path.
2. **Import / diff read paths** — decide the boundary deliberately, then apply it uniformly:
   - resolve relative paths against the workspace root, not the process CWD, so the same call means the same thing from either pane;
   - reject anything that escapes the workspace or the configured data dir after symlink resolution;
   - state the boundary in each tool description, so an agent knows before it tries rather than after it is refused.
3. Whichever boundary is chosen, pin it with tests that assert a traversal attempt (`../../.config/autostart`, an absolute path outside the root, a symlink pointing out) is refused — the #249 tests are the pattern.

## Acceptance

- [ ] No MCP tool accepts a caller-supplied directory, and no caller-supplied path escapes the agreed boundary.
- [ ] Traversal, absolute-outside-root and symlink-out cases are all refused, each with a test.

## Related

- #249 (`download_paper` `output_dir`, fixed in #265), #247 (whose new acquisition verbs must not inherit this shape).
