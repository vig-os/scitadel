---
type: issue
state: open
created: 2026-09-30T00:26:07Z
updated: 2026-09-30T00:26:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/255
comments: 0
labels: feature, effort:small, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:20.209Z
---

# [Issue 255]: [spike(acquire): chromiumoxide PDF/SI capture + SWITCH edu-ID SSO in a dedicated profile](https://github.com/vig-os/scitadel/issues/255)

An empirical spike that must be done before S4 (ADR-007 `docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §5, §6).

**Questions**
1. Does `chromiumoxide` `Fetch`-domain body capture (`Fetch.getResponseBody`) work for PDFs opened in Chrome's built-in viewer, and with publisher service workers?
2. Does an in-page `fetch(url, {credentials:'include'})` via `Runtime.evaluate` work for same-origin PDF and SI URLs at ACS, RSC, Elsevier, T&F and SNMMI?
3. Does SWITCH edu-ID / Shibboleth SSO complete in a **fresh dedicated profile** launched with `--remote-debugging-pipe`? Which IdP and WAYF hosts must the origin allowlist include?

**Deliverable:** a short report with working code snippets, filed on this issue. The findings update ADR-007 §5 if needed.

Refs: #247, #230
