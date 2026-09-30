---
type: issue
state: open
created: 2026-09-30T00:26:08Z
updated: 2026-09-30T00:26:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/256
comments: 0
labels: feature, effort:large, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:19.571Z
---

# [Issue 256]: [feat(acquire): S4 — tier 4 one-click 'save what I opened' in a dedicated browser profile](https://github.com/vig-os/scitadel/issues/256)

Slice S4 of ADR-007 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §5).

**Scope** (behind the `browser-session` cargo feature)
- A dedicated Chrome profile, launched by scitadel over `--remote-debugging-pipe`.
- A per-publisher origin allowlist, enforced via `Fetch.enable`, with DOIs pre-resolved through the Handle API.
- Fetches happen inside the page, so cookies never leave the browser.
- From the page the human opened, save: full text, SI, tables (JSON; S4 introduces the HTML table extractor), and figure references.
- Run the post-fetch identity check, then close scitadel's own tab. One human action per work, and the pacer applies.

**Acceptance**
- [ ] A redirect to a non-publisher origin is blocked (test).
- [ ] Session cookies can't be retrieved over the connection (`get_cookies` is linted out).
- [ ] A saved work produces artefacts with `access_basis = subscription_read` and `auth_context = session:<publisher>`.

**Depends on:** S2 (253), #248, #249, the browser spike (255).
Refs: #247, #230
