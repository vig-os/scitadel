---
type: issue
state: open
created: 2026-09-30T00:26:10Z
updated: 2026-09-30T00:26:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/257
comments: 0
labels: feature, effort:medium, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:18.977Z
---

# [Issue 257]: [feat(acquire): S5 — SI discovery at scale + HTML tables for OA landing pages](https://github.com/vig-os/scitadel/issues/257)

Slice S5 of ADR-007 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §3).

**Scope**
- SI from DataCite `IsSupplementTo` (T&F on Figshare, plus Zenodo and Dryad).
- Publisher-hosted SI (ACS, RSC), fetched through the publisher's pacer bucket, with magic-byte checks.
- HTML tables → JSON for OA landing pages, reusing S4's extractor.

**Acceptance**
- [ ] Pacing covers SI fetches (tested).
- [ ] An HTML landing page is never counted as SI.
- [ ] T&F supplements are found via DataCite.

**Depends on:** S3 (254) and S4 (256).
Refs: #247, #234
