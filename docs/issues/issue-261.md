---
type: issue
state: closed
created: 2026-10-02T12:21:36Z
updated: 2026-10-02T23:19:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/261
comments: 2
labels: bug, effort:medium, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:42.867Z
---

# [Issue 261]: [fix(acquire): publisher classification is a per-consumer prefix table, so 249 papers get a route verdict that was never evaluated](https://github.com/vig-os/scitadel/issues/261)

## Problem

The acquisition ladder's verdict depends on classifying a DOI's publisher from its prefix, and the classification is a hand-maintained table that only covers publishers with a TDM API. Everything else comes out as "publisher unknown", and the run note then asserts **"no TDM route available"** — a statement the ladder has not actually established.

## Evidence (raid, 2026-10-02)

Fetch run over a 1,889-DOI corpus. Failure counts by the publisher the consumer classified independently:

| classified publisher | failures | what the ladder's own note said |
|---|---:|---|
| `acs` | 190 | "Publisher unknown; no TDM route available" |
| `elsevier` | 90 | correct (TDM key missing) |
| `rsc` | 32 | "Publisher unknown; no TDM route available" |
| `wiley` | 11 | correct (TDM token missing) |
| `oup` | 10 | "Publisher unknown; no TDM route available" |
| `springer` | 7 | correct (TDM key missing) |
| `mdpi` | 6 | "Publisher unknown; no TDM route available" |
| `pnas` | 3 | "Publisher unknown; no TDM route available" |
| `nist` | 1 | "Publisher unknown; no TDM route available" |

A publisher-blind note is not cosmetic. It is the text a human reads to decide what to do, and for 249 papers it names a route that was never evaluated.

## The same class of bug, hit twice in one consumer

raid hit this **independently in two places** on the same day, which is the argument for fixing it once here:

- `tools/fetch/fulltext.py` had a `PUBLISHER_PREFIXES` map covering only the 10 TDM registrants.
- `scripts/build-acquisition-inventory.py` had its own, wider map (added because the first was too narrow).

Two tables, written from scratch, already diverging. Any ladder that infers publisher identity from a local prefix table pushes that table onto every consumer, and they will all drift.

There is also a **segmented-identifier** trap in the same area: some registrants put a slash inside the DOI suffix (OUP `10.1093/nar/<id>`, IOP `10.1088/1742-6596/<article>`, ChemRxiv `10.26434/chemrxiv.<id>/v1`, De Gruyter `10.17308/<journal>.<vol>/<page>`). raid's first fragment filter banned the slash outright and discarded nine real `10.1093/nar` DOIs before the rule was scoped to the registrants that genuinely have one. A naive "suffix has no slash" check is wrong; a naive "first slash splits prefix from suffix" check is also wrong for these.

## Proposal

1. **One exported, versioned prefix→publisher table**, so consumers import it instead of re-deriving it. Registrant prefixes are stable and public (doi.org prefix registry); a checked-in table with a provenance note beats N hand-maintained copies.
2. **Distinguish three verdicts, not two**: `no_tdm_route_because_publisher_unknown` ≠ `no_tdm_route_available` ≠ `tdm_available_but_key_missing`. Only the third should ever produce a "register a key" instruction.
3. **A DOI-shape validator that knows segmented identifiers** — balanced parens for Elsevier compact DOIs (`10.1016/0022-1902(65)80271-8`), the ACS pre-2008 form with a colon in the suffix (`10.1023/a:1006721613253`), and a slash check scoped to the registrants above. Expose it so consumers stop each re-inventing it; see the companion issue on half-parenthesised DOIs reaching a fetch queue.

## Acceptance

- [ ] A consumer can obtain the publisher for a DOI (or an explicit `unknown`) from an exported table, without writing its own prefix list.
- [ ] A note never asserts "no TDM route available" for a publisher the ladder did not classify.
- [ ] The segmented-identifier DOIs listed above round-trip through parse/validate unchanged, and a half-parenthesised Elsevier DOI is rejected with a reason.

## Related

- #252 (work/artefact model — publisher belongs on the work), #260 (OA routes missed for the same class of papers).
---

# [Comment #1]() by [gerchowl]()

_Posted on October 2, 2026 at 04:21 PM_

Work in progress: #271 (validator + publisher table, closes this and #262). Started from `feat/261-doi-validator-and-publisher-table`.

---

# [Comment #2]() by [gerchowl]()

_Posted on October 2, 2026 at 11:19 PM_

Fixed by #272 (merged into `dev`).

- `scitadel_core::publisher` exports one versioned registrant-prefix table. `classify_publisher` returns `Known` or an **explicit** `Unknown` carrying the offending prefix — never a default that reads as knowledge.
- `route_verdict` splits the two-valued verdict into four, keeping *unknown* apart from *known absent`. The acceptance criterion holds: **no note ever asserts "no TDM route available" for a publisher the ladder never classified**, pinned by a test that walks a plausible DOI corpus.
- `HostKind` marks preprint servers separately from publishers (they are free by construction), which is the distinction #260's route list depends on.

Two things found while working this that the issue did not mention:

1. **`scitadel-export` had a second `normalize_doi`** handling the `doi:` CURIE that `.bib` files carry but *not* `dx.doi.org`, while core's handled the reverse. So a `10.1234/x` keyed off a resolver URL and the same DOI keyed off a CURIE did not converge. One implementation now handles all six prefix forms.
2. The segmented-identifier trap is real: the validator deliberately has **no** "suffix has no slash" rule and **no** colon rule, because OUP/IOP/ChemRxiv/De Gruyter and ACS pre-2008 need them.

Verified by removing each new rule and confirming the matching test fails.

