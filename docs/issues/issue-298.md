---
type: issue
state: closed
created: 2026-10-08T10:53:10Z
updated: 2026-10-08T23:26:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/298
comments: 1
labels: bug, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-09T08:09:14.819Z
---

# [Issue 298]: [fix(scanner): OpenAlex's 'locations[].version' files PubMed citation pages as the version of record, so bioRxiv PDFs never rank](https://github.com/vig-os/scitadel/issues/298)

## Problem

Found while fixing #260's dominant cause (PR — the `similarity-checking` filter + ADR-007 §3 fetch-order tie-break). With that done, #260 clause 1 is still 0/3 for bioRxiv, and the cause is not the ranker.

For a `10.1101/*` preprint, OpenAlex's `best_oa_location` is the work's **PubMed citation page**, and OpenAlex files it:

```
locations[].version = publishedVersion
```

OpenAlex also carries `primary_location` / `locations[]` for the biorxiv.org PDF. So the same paper produces two candidates:

- pubmed.ncbi.nlm.nih.gov/<pmid> — "publishedVersion"
- biorxiv.org/content/...v1.full.pdf — the bioRxiv transform's own `preprint`

We rank `VersionOfRecord` strictly above `AuthorManuscript`/`Preprint` (wave 2's settled rule). So OpenAlex's `publishedVersion` on a PubMed page **always** outranks the bioRxiv candidate. Rank 1 for all three `10.1101` DOIs is a `pubmed.ncbi.nlm.nih.gov` abstract page — a 203 of 5590 bytes of HTML — and the PDF fetch never happens.

pubmed.ncbi.nlm.nih.gov/<pmid> is a **citation record**, not a reading location. This is "a URL that serves metadata about a work, not the work" — the same category error as a Crossref `similarity-checking` link, filed differently: it arrives as an OpenAlex `version` word rather than a `similarity-checking` `intended-application`.

## Why this is §3 vocabulary work, not ranker work

The ranker cannot be told to distrust OpenAlex's `version` word — Crossref/DataCite versions are that word too, and some of them are right. The honest distinguishing feature is *what the URL serves*: a tabular/citation record vs the article body. That is what `Candidate::kind` needs to carry, and without a `kind` field there is nowhere for the information to live.

Wave 2 got this as far as it could without the field:

- `Rank` is a two-key tuple so a caller could filter without touching the comparator.
- `access_basis` did **not** grow a second producer, precisely because #261's rule requires one place to put each kind of claim.

The equivalent move now is a `Candidate::kind` that says which of those it is, and the version vocabulary that only asserts `VoR` when the URL is a reading location.

## Evidence

- `gh issue` on #260: the three `10.1101` DOIs all record `pubmed.ncbi.nlm.nih.gov` at rank 1 with HTTP 203.
- Wave-2 manifest `docs/pull-requests/pr-292.md` measured OpenAlex like this without listing `primary_location` behaviour when it is a bioRxiv record.

## What a fix looks like

1. `Candidate::kind` distinguishes `ArticlePdf`, `ArticleHtml`, `LandingPage`, `CitationRecord`, `Manifest`, `Unknown`.
2. A `publishedVersion` word from an **identity-only** source (`OpenAlex`/`Unpaywall` named `best_oa_location` etc.) does not raise a `CitationRecord`/`LandingPage` URL to `VoR`. Either it becomes `Unstated` (kept, rung, reportable), or it drops to the publisher landing-page step.
3. The bioRxiv/medRxiv/ChemRxiv DOI-transform candidate, which cites the transformed host directly, is then top-ranked for preprints — clause 1 passes.

## Acceptance

- [ ] For every `10.1101` preprint DOI, a `viorxiv.org`/`pubmed.ncbi.nlm.nih.gov` citation page is not at rank 1.
- [ ] `#260` clause 1 measures 3/3 on the live contract suite's preprint probe.
- [ ] `Candidate::kind` exists and is checked before a candidate is placed at rank 1.

Refs: #260, #254, #293
---

# [Comment #1]() by [gerchowl]()

_Posted on October 8, 2026 at 11:26 PM_

Implemented in #300. `CandidateKind` exists, is derived from each source's own role naming (OpenAlex `pdf_url`/`landing_page_url` plus the `pmid:`/`pmh:` identifier prefix, Unpaywall `url_for_pdf`, Crossref `link[].content-type`, Europe PMC `documentStyle`), and `CitationRecord`/`Manifest` are excluded while `LandingPage` is kept (ADR-007 §3 step 6).

Third acceptance box is **not** met and is tracked on #260: the residual is an ADR-007 §3 precedence question, not a bug.

