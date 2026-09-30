---
type: issue
state: open
created: 2026-09-30T00:26:13Z
updated: 2026-09-30T00:26:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/259
comments: 0
labels: feature, effort:large, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:17.727Z
---

# [Issue 259]: [feat(acquire): S7 — gated tier-4 bulk drain + TUI acquisition queue](https://github.com/vig-os/scitadel/issues/259)

Slice S7 of ADR-007 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §5).

**Scope**
- Bulk queue draining, **only** for publishers whose `tdm_authorisation` has `allows_bulk_session = 1`.
- Started by a human from the TUI or CLI, never through MCP.
- Per-publisher authenticated probes detect login; a session expiring mid-drain pauses that publisher only.
- A TUI queue view with OSC 8 login links, and caps that are visible.

**Acceptance**
- [ ] The tier-4 caps (≥30 s + jitter, ≤50 works per publisher, ≤150 in total) are enforced and shown.
- [ ] Draining is refused without an authorisation.

**Depends on:** S4 (256), S6 (258), and a library agreement (ETH Library e-resources and TDM contact).
Refs: #247, #230
