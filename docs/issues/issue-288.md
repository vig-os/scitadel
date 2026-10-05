---
type: issue
state: open
created: 2026-10-04T20:47:20Z
updated: 2026-10-04T20:47:20Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/288
comments: 0
labels: bug, effort:small, priority:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T07:53:08.486Z
---

# [Issue 288]: [fix(adapters): no size cap on a fetched body, so an HTTP route can write a 2 GB blob](https://github.com/vig-os/scitadel/issues/288)

## Problem

ADR-007 §1 specifies size caps per kind (full text 100 MB, SI 250 MB, figure/table 20 MB). `scitadel-adapters/src/magic.rs` and `blobs.rs` now implement `too_large` and `cap_for_kind`, and #286 wired the writer — but the check only runs on **local files**, through `scan` and `attach`.

`download.rs` has **no size cap on a fetched body**. A route that reaches a publisher serving a multi-gigabyte file today writes the whole thing: bytes to a staging file, then a `blobs` row, then an artefact pointing at it. The cap is applied on the next `scan`, which is never, because the file is already filed.

Order matters and is already specified: ADR-007 §1 says **size cap, then magic, then hash** — a 2 GB file is never hashed to learn it was never going to be filed. That ordering is implemented for local files and simply absent on the fetch path.

## Why it matters beyond disk

- **It is the one DoS a user can trigger by pointing scitadel at a paper.** Everything else in the ladder is bounded by the pacer and the 24 h caps; this is not.
- The cap is keyed on **kind**, not format, specifically so a big workbook cannot be filed as a figure to get under a smaller cap. That reasoning is defeated if the fetch path is uncapped: the kind is decided *after* the body arrives, so an unbounded download gets filed under whatever kind wins.
- The staged write is deliberately outside the transaction (that is what makes gc's 24 h grace period necessary), so an oversized body occupies real disk before anything can object — and a `Content-Length` lie means even a pre-flight check on the header is not sufficient.

## Proposal

1. Cap during the read, not after: stop pulling bytes once the cap for the expected kind is exceeded, and record `too_large` on `acquisition_attempts` with the observed size and the cap. Never write the body to the store.
2. **Do not trust `Content-Length`.** The oversize case is a common way to elicit the behaviour, so the enforced limit must come from the byte count actually read. A declared length may be used to refuse *early*, but never to raise the limit.
3. If `Content-Length` is absent, stream with a running total. A chunked response must be bounded too.
4. Reuse `cap_for_kind` and `record_attempt` — this is the same fix as the local path, called from a second place. `bad_magic` is already wired this way.
5. Test with `wiremock` serving a body larger than the smallest cap (20 MB is heavy; make the cap injectable for the test rather than serving 100 MB).

## Acceptance

- [ ] A fetched body above the cap is never written to the blob store, and records `too_large`.
- [ ] A lying `Content-Length` cannot raise the enforced limit.
- [ ] A chunked response with no `Content-Length` is bounded.
- [ ] `scan` and the fetch path share one implementation of the rule.

## Notes

- Not a security advisory: it requires the user to point scitadel at a work whose route serves a huge file, which needs no attacker. Filed as a robustness bug because the cap exists in the ADR and is half-implemented, which is worse than either state.
- S3's SI routes (Europe PMC / PMC OA / DataCite / Figshare) must call this too — SI is the largest kind at 250 MB, so it is where the cap will actually be exercised.

Refs: #253, #285, #286, ADR-007 §1.
