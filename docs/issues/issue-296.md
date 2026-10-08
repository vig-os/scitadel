---
type: issue
state: open
created: 2026-10-07T20:13:14Z
updated: 2026-10-07T20:13:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/296
comments: 0
labels: bug, priority:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-08T08:09:36.474Z
---

# [Issue 296]: [europepmc.org PDFs 403 behind Cloudflare, so an Europe PMC candidate names a location the fetch leg cannot reach](https://github.com/vig-os/scitadel/issues/296)

## Problem

Found while implementing #254 wave 3. Europe PMC's `fullTextUrl` names the PDF as:

```
https://europepmc.org/articles/PMC7759461?pdf=render
```

Measured 2026-10-07, reproduced independently:

```
curl -A 'scitadel/0.1' 'https://europepmc.org/articles/PMC7759461?pdf=render'
  -> 403  (Cloudflare interstitial, on every User-Agent and Accept tried)
curl 'https://www.ebi.ac.uk/europepmc/webservices/rest/PMC7759461/fullTextXML'
  -> 200
```

So wave 3 can **name** a Europe PMC candidate correctly, and the fetch leg will record `Inconclusive` when it goes to get it. The metadata pass is unaffected — it only reads `europepmc.org`'s *search* API, which answers fine.

Wave 3 was merged deliberately with this reported rather than worked around: substituting `fullTextXML` would file JATS XML where the plan says PDF, a different artefact and a different manifest row.

## What needs deciding

This is a fetch-path question with three honest options, and none is a pure bug fix:

1. **Prefer a working endpoint.** Europe PMC's REST API also serves PDFs via `api/fulltextRepo` / the EBI mirror. If one serves the same bytes, name it. Cost: the candidate's `route` then describes a host other than `europepmc.org`, so `leg_of` and the plan line get more honest or more confusing depending on which you pick.
2. **Accept `Inconclusive` and move on.** Honest, and the PMC-OA bucket already covers much of the same corpus over a path that answers 200 — with *better* version evidence, since `is_manuscript` is PMC's claim about the stored file rather than a field on a landing page.
3. **Route to JATS deliberately.** File `fullTextXML` as its own artefact kind (ADR-007 §3 step 1 already names JATS, and S5 wants `table-wrap` extraction). This makes the 403 a non-problem, but needs `Candidate::kind` to distinguish PDF from JATS XML, which #288's SI work wants anyway.

**I lean to 3**, because ADR-007 already asks for JATS and the current gap is that nothing fetches it. But it is a scope call, not a correction.

## Acceptance

- [ ] A Europe PMC OA work reaches a fetchable document, or its `Inconclusive` is documented as expected rather than surprising.
- [ ] No claim anywhere that Europe PMC full text is retrievable by PDF unless verified at the time of the claim.
- [ ] `Candidate` can distinguish a PDF from a JATS XML payload if option 3 is taken.

Refs: #254, #260, #288
