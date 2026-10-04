---
type: issue
state: open
created: 2026-10-02T12:20:49Z
updated: 2026-10-03T22:38:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/260
comments: 1
labels: bug, effort:medium, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-04T07:34:06.244Z
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


