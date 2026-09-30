---
type: issue
state: open
created: 2026-09-30T00:26:04Z
updated: 2026-09-30T00:26:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/253
comments: 0
labels: feature, effort:large, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:21.604Z
---

# [Issue 253]: [feat(acquire): S2 — acquire / coverage / action_list + identity checks + queue](https://github.com/vig-os/scitadel/issues/253)

Slice S2 of ADR-007 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §1, §2, §3, §5, §6).

**Scope**
- CLI and MCP: `acquire` (idempotent, with `--resume` and `--dry-run`), `coverage`, `action_list`, and `acquire_queue_add(paper_ids)`.
- Identity checks before and after fetching, for HTTP routes.
- The OSTI route.
- `scan` / `attach` for manually dropped-in files, the manifest mirror, and `gc` for unreferenced blobs.
- The TUI and `read_paper` move onto artefacts (with the untrusted-content envelope), and legacy writes stop.

**Acceptance**
- [ ] `action_list` counts equal the missing totals in `coverage`.
- [ ] A re-run makes no network calls for artefacts already held.
- [ ] No artefact is ever filed under a mismatched identity.
- [ ] raid's `p1`/`p3` NDJSON acquisition fields import via `acquire --from works.ndjson`.

**Depends on:** S1 (252).
Refs: #247, #234
