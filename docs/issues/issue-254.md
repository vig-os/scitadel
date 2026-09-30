---
type: issue
state: open
created: 2026-09-30T00:26:05Z
updated: 2026-09-30T00:26:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/254
comments: 0
labels: feature, effort:large, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:21.101Z
---

# [Issue 254]: [feat(acquire): S3 — OA routes: resolve-then-rank, Europe PMC JATS, PMC OA dataset, Crossref licences](https://github.com/vig-os/scitadel/issues/254)

Slice S3 of ADR-007 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §3).

**Scope**
- One metadata pass (OpenAlex `locations[]`, Unpaywall, Crossref, DataCite, Europe PMC), then ranking by version and licence.
- Europe PMC `fullTextXML` + `supplementaryFiles`, gated on `isOpenAccess` / author-manuscript status.
- The `pmc-oa-opendata` S3 dataset, replacing `oa.fcgi`, which was retired on 2026-08-25.
- Parse Crossref `license[]` properly: only allow-listed CC licences already in force count as OA.
- JATS tables → structured JSON.
- Move the acquisition metadata adapters onto `PacedClient`.

**Acceptance**
- [ ] The number of JATS `table-wrap` elements equals the number of table artefacts.
- [ ] An OA version of record is preferred over a preprint.
- [ ] Licence fields are populated per artefact.

**Depends on:** S1 (252). Can run in parallel with S4.
Refs: #247, #234
