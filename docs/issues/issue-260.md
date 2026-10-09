---
type: issue
state: open
created: 2026-10-02T12:20:49Z
updated: 2026-10-08T23:26:45Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/260
comments: 3
labels: bug, effort:medium, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-09T08:09:15.248Z
---

# [Issue 260]: [fix(acquire): preprint and publisher-direct OA routes are missing — 52 free papers reported unreachable (bioRxiv 3/3 missed)](https://github.com/vig-os/scitadel/issues/260)

## Problem

A preprint on bioRxiv or medRxiv is free to download by construction. A run of the acquisition ladder over a real corpus missed **all of them**, reporting each as unreachable.

## Evidence (raid, radiometal radiopharmaceutical datasets)

Fetched 2026-10-02 with the current route ladder (OpenAlex OA → OSTI → publisher TDM → needs-user). Three bioRxiv preprints, three failures:

| DOI | outcome |
|---|---|
| `10.1101/2025.06.14.659707` | `needs_user_fetch` |
| `10.1101/2024.11.19.624167` | `needs_user_fetch` |
| `10.1101/2024.10.10.615955` | `needs_user_fetch` |

All three resolve at doi.org and all three have a free full text at `biorxiv.org`. The failure is not access, it is that the ladder has **no preprint route**: route A depends entirely on OpenAlex returning `best_oa_location.pdf_url`, and when that field is null the paper is declared unreachable even though the publisher serves it openly.

The same root cause hits open-access journals that are free or partly free: in the same run, **52 papers from publishers that are not paywalled** were reported unreachable — MDPI (fully OA), RSC, OUP/NAR, PNAS, NIST. For example `10.3390/pharmaceutics14051098` (MDPI, CC BY), `10.18434/m32154` (NIST, public domain), `10.1073/pnas.2415658122`, `10.1093/nar/gkv1075`.

## Why this matters for corpus acquisition

#234's premise is that coverage is reported per work with a grouped list of what still needs a human. If "unreachable" is the verdict for 52 free papers, the human list is wrong in the worst direction: it sends someone to request documents they could simply download, and it hides the fact that the *route* is missing rather than the *access*.

## Proposal

1. **Direct preprint routes.** bioRxiv/medRxiv (`/content/<prefix>v<id>.full.pdf`), arXiv (`/pdf/<id>`), ChemRxiv (`10.26434`), and SSRN. These are deterministic transforms of the DOI, not searches — cheap and reliable, and they do not depend on an index being complete.
2. **Publisher-direct OA routes as a fallback when OpenAlex has no location.** MDPI, RSC, Frontiers, PLOS, MDPI, and NIST all serve OA content from a derivable URL or a per-prefix landing page. #253 covers Europe PMC / PMC JATS / Crossref licences; this is the complementary *publisher-direct* half, and the measurement above is the argument for shipping it.
3. **Distinguish "no route tried" from "no access".** The per-work status should say which routes were attempted and why each was skipped. "Unreachable" should never be reachable-by-download papers.

## Acceptance

- [ ] Given a bioRxiv/medRxiv/arXiv/ChemRxiv DOI, `acquire` obtains the PDF with no network index lookup, or says which of those servers it tried.
- [ ] The 52-DOI OA-publisher set above re-runs with ≥90% obtained, or each residual is a genuinely paywalled paper.
- [ ] Coverage output distinguishes `no_route_tried` from `no_access`, and the action list only contains the latter.

## Related

- #252 (work/artefact model), #253 (OA routes), #254 (tier-4 session capture). This is the measured gap in #253's scope: OpenAlex-only OA resolution.
- Consumer context: raid's own ladder is `tools/fetch/fulltext.py` in gerchowl/raid; the run above is reproducible from it.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 3, 2026 at 10:38 PM_

Partially addressed by #280 (merged). **Not closing** — one acceptance criterion is not met and is not achievable here.

## Done

- **Preprint routes.** bioRxiv, medRxiv and arXiv DOIs now resolve to a PDF with **zero index lookups**, ahead of the OpenAlex/Unpaywall legs. ChemRxiv returns no candidate and says why, rather than guessing a URL.
- **"No route tried" is now distinguishable from "no access"** in the data, via `pending` / `error` / `unavailable` derived from per-leg outcomes. An index answering 200 with no OA location can no longer produce `unavailable` — which is what mislabelled the three bioRxiv preprints.

One assumption in this issue was **wrong**, and measurement corrected it: `10.1101` covers bioRxiv *and* medRxiv, and `www.medrxiv.org` serves **403 to every User-Agent** because its `/content/<doi>` path does not exist there. `www.biorxiv.org` serves both. So there is no server choice to make, and no candidate ever names medrxiv.org.

## Not done

**The 52-DOI OA-publisher criterion is not met.** MDPI, RSC, OUP/NAR, PNAS and NIST are untouched: their URLs are **not derivable from a DOI** — NIST's `nvlpubs.nist.gov` path and OUP's `academic.oup.com` article-pdf path both carry data the identifier does not. This is #253/#254's index-based work.

What *did* change for those papers: they now record `pending`/`error` instead of a no-access verdict, so the human action list stops sending someone to request documents they could simply download — which was this issue's "why this matters". The remaining work is obtaining them, not labelling them.

Also still open: the `coverage`/`action_list` **commands** do not read `acquisition_state` yet (#253). #280 fixes the data they will read.

Keeping this open for the 52-DOI half.


---

# [Comment #2]() by [gerchowl]()

_Posted on October 5, 2026 at 01:58 PM_

**Correction to my previous comment on this issue.** I wrote that six of ten unresolvable DOIs were preprints/repository DOIs "that Crossref 404s", and concluded that DataCite — not Crossref — holds the preprint-server prefixes. **That is wrong**, and the re-probe in #292 corrected it:

| DOI | Crossref | DataCite |
|---|---|---|
| `10.1101/…` (bioRxiv/medRxiv) | **200** | 404 |
| `10.26434/chemrxiv.…` (ChemRxiv) | **200** | 404 |
| `10.18434/m32154` (OSTI) | 404 | **200** |
| `10.5281/zenodo.…` | 404 | **200** |

`10.1101` and `10.26434` are registered to **Crossref** — `10.26434` has 56,845 `posted-content` records as a Crossref member. Crossref has taken over bioRxiv/medRxiv registration. DataCite holds the *repository* DOIs, not the preprint servers.

My method was the flaw, not just the conclusion: I inferred registry ownership from which API 404'd, on a sample where half the DOIs were ones I had invented. A 404 is evidence that an API lacks a record, not evidence about who owns the prefix.

**The conclusion survives on half the evidence, which is the half that matters:**

- `10.18434` (this issue's own example) and `10.5281` are **only resolvable by DataCite**. Crossref has no second opinion for them at all, so an identity chain without DataCite cannot corroborate them. That case is now covered — #292 adds the adapters and the chain.
- The `10.1101` papers were **never failing on identity**. Crossref answered 200 for all three. They failed on **routing**, which #280 fixed with the deterministic preprint transforms. So this issue's premise — that preprints were missed for want of a lookup — was half wrong: they were missed for want of a *URL*.

The 52-DOI publisher set in the original report is untouched by any of this and still needs the index routes.


---

# [Comment #3]() by [gerchowl]()

_Posted on October 8, 2026 at 11:26 PM_

## Measurement after #299 and #300 (live contract suite, 2026-10-09)

Three changes landed since this issue's last measurement, and two of them are ranker work: #299 (Crossref `similarity-checking` links excluded, and ADR-007 §3's fetch order applied as the tie-break after version+licence), then #300 (candidate kinds, so a PubMed citation page cannot wear a publication's `version` word and win on version dominance).

| | bytes obtained | obtained **as a PDF** |
|---|---|---|
| original finding (2026-10-02) | 3 bioRxiv preprints: 3/3 missed | — |
| after #297's harness | 4 / 16 | 1 / 16 |
| after #299 (ranker) | 15 / 16 | 6 / 16 |
| after #300 (candidate kinds) | 13 / 16 | **7 / 16** |

The byte number *fell* and that is the honest direction: two rows previously "obtained" a PubMed abstract page, which is not an article. Both now report the publisher's real 403.

Clause 1 of the three bioRxiv DOIs: a PubMed citation page at rank 1 went **2/3 → 0/3**, and a bioRxiv PDF at rank 1 went **0/3 → 1/3**.

## Clause 1 now needs an ADR decision, and it belongs here

For `10.1101/2025.06.14.659707`, the live OpenAlex record offers three copies:

```
best_oa_location  doi:…                       acceptedVersion
locations[1]      pmid:40667369               publishedVersion   <- PubMed page
locations[2]      pmh:oai:pubmedcentral:12262699  submittedVersion
```

#300 excludes the PubMed page. What then wins is **Europe PMC's CC-BY author manuscript at ADR-007 §3 step 1** — because §3's own ranking says `OA VoR > AM > preprint`, and §3's fetch order puts Europe PMC first. Both of those are the ADR's stated rules, and both are load-bearing elsewhere.

So **clause 1 asks for a document the ADR deliberately deprioritises.** Two coherent options:

**(a) Amend §3 so the author-manuscript rung is for works without an OA version of record.** Europe PMC's `am` copy would rank above the preprint copy only when no OA VoR exists. For a bioRxiv preprint — where bioRxiv *is* the publication — the transform's PDF is the better artefact.

**(b) Leave §3 alone and amend clause 1 of this issue.** The library is taking an author manuscript from an OA step-1 source, which is defensible and is what the ADR asks for; clause 1's 0/3 was about the PubMed page, which is fixed.

I lean (a), because clause 1's wording — "obtains the PDF with no network index lookup" — is about determinism and the pacer budget, and a DOI transform is the one route that spends neither. But it is a real choice about what artefact the library wants, and I would rather it be made than implied.

## On clause 2's "52-DOI set"

The set this issue calls "above" is **not enumerated in the issue** — it names seven DOIs. #297's harness carries 16, each tagged with its provenance (`Issue260` or a recorded live query), and `LIVE_QUERY_PROVENANCE` records the queries. Until someone can name the 52, ≥90% cannot be measured as written. If the original run's DOI list is recoverable from raid's `tools/fetch/fulltext.py`, adding it to `crates/scitadel-adapters/src/oa_live.rs` would close this properly.

