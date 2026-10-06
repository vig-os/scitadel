---
type: issue
state: open
created: 2026-09-30T00:26:05Z
updated: 2026-10-05T10:07:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/254
comments: 1
labels: feature, effort:large, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-06T08:17:52.722Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on October 5, 2026 at 10:07 AM_

Measured the open question from #285 before starting S3: **is the identity matcher's strict corroboration rule too tight for real data?**

The rule records `ok` only when a title match is corroborated by a year within ±1 **or** a first-author surname. If real metadata routinely lacks both, every check would be `unverified` and the rule would be wrong.

## What I measured

20 DOIs (mostly from this repo's own fixtures and earlier issue evidence) against Crossref's API, polite pool, recording whether each record carries a title, a year and a first author.

```
resolved 10/20
  title present        10/10
  year present         10/10
  first author present 10/10
  missing a corroborator (no year OR no author): 0
```

**So pre-fetch corroboration is viable.** The resolved side is a metadata index and it is well populated; a strict rule is not too tight there. That was the question #285 left open, and it is settled in the rule's favour.

Small sample and biased towards well-known records — it establishes viability, not a rate.

## Two things the measurement did *not* settle, one of which I had wrong

**1. It is the wrong population for post-fetch.** This measures a metadata *index*. The post-fetch check reads a **served page or a PDF**, which is a different population entirely. #285 argued from first principles that a PDF post-fetch check can only ever be `unverified` or `mismatch`, because a PDF carries no year and no author to corroborate its `/Title`. Nothing here contradicts that, and nothing here could have — but it also does not test it. So: **pre-fetch is corroborated in practice; post-fetch remains structurally uncorroborable**, and the two need separate treatment rather than one shared expectation.

**2. Six of the ten unresolvable DOIs were preprints or repository DOIs** — bioRxiv/medRxiv (`10.1101`), ChemRxiv (`10.26434`), Zenodo (`10.5281`), OSTI (`10.18434`), and the made-up IOP/De Gruyter ones.

That is not a sampling artefact. **Those prefixes are DataCite's, not Crossref's.** Crossref returns 404 for all of them.

So the identity chain's second and third sources — Crossref and DataCite — are not interchangeable extras. Crossref alone cannot corroborate a preprint identity at all, and #260's three missed papers were **all** preprints. A preprint corpus is precisely the case where a Crossref-only chain has no second opinion, which is the exact gap S1's preprint routes were built to close on the *fetch* side and which remains open on the *identity* side.

**This makes DataCite required for S3, not optional** — which is already how #285 described it ("declared as an identity source but unreachable"), but now with evidence rather than inference.


