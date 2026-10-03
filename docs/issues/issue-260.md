---
type: issue
state: open
created: 2026-10-02T12:20:49Z
updated: 2026-10-02T12:20:49Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/260
comments: 0
labels: bug, effort:medium, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:43.247Z
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
