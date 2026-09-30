---
type: issue
state: open
created: 2026-09-30T00:26:12Z
updated: 2026-09-30T00:26:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/258
comments: 0
labels: feature, effort:medium, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:18.389Z
---

# [Issue 258]: [feat(acquire): S6 — tier 2 sanctioned TDM (Elsevier, Wiley, Springer platform) + tdm_authorisations](https://github.com/vig-os/scitadel/issues/258)

Slice S6 of ADR-007 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §3, §5).

**Scope**
- Elsevier Article Retrieval + Object APIs (API key, plus `insttoken` off campus).
- Wiley TDM token.
- Springer platform download at ≤1 req/s.
- A `tdm_authorisations` table and CLI for recording them.
- IP-based access only where a publisher's TDM terms permit it.
- A `Secret` newtype; keys are scoped to their own publisher's bucket, with no redirects to other buckets.

**Acceptance**
- [ ] The canary non-leakage test passes across MCP, CLI, TUI, logs, the manifest, the DB and error paths.
- [ ] Missing keys produce `tdm_key_missing` entries in `action_list`.

**Depends on:** S1 (252), #248.
Refs: #247, #230
