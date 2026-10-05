---
type: issue
state: closed
created: 2026-09-30T00:26:04Z
updated: 2026-10-04T21:54:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/253
comments: 1
labels: feature, effort:large, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T07:53:09.271Z
---

# [Issue 253]: [feat(acquire): S2 — acquire / coverage / action_list + identity checks + queue](https://github.com/vig-os/scitadel/issues/253)

Slice S2 of ADR-007 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §1, §2, §3, §5, §6).

**Scope**
- CLI and MCP: `acquire` (idempotent, with `--resume` and `--dry-run`), `coverage`, `action_list`, and `acquire_queue_add(paper_ids)`.
- Identity checks before and after fetching, for HTTP routes.
- The OSTI route.
- `scan` / `attach` for manually dropped-in files, the manifest mirror, and `gc` for unreferenced blobs.
- The TUI and `read_paper` move onto artefacts (with the untrusted-content envelope), and legacy writes stop.

**Acceptance**
- [ ] `action_list` counts equal the missing totals in `coverage`.
- [ ] A re-run makes no network calls for artefacts already held.
- [ ] No artefact is ever filed under a mismatched identity.
- [ ] raid's `p1`/`p3` NDJSON acquisition fields import via `acquire --from works.ndjson`.

**Depends on:** S1 (252).
Refs: #247, #234
---

# [Comment #1]() by [gerchowl]()

_Posted on October 4, 2026 at 09:54 PM_

Slice S2 complete, in five merged PRs. Every acceptance criterion is met and the scope is finished.

| Criterion | Where | Pinned by |
|---|---|---|
| `action_list` counts equal the `coverage` missing totals | #283 | one shared read; `accounted()` sums over the partition, and the test parses the headline number out of both renderings |
| A re-run makes no network calls for artefacts already held | #284 | `request_count()` compared **across** the second run, after asserting the first *did* fetch |
| No artefact is ever filed under a mismatched identity | #285 | `artefacts` compared **row-for-row**, and no blob written |
| raid's `p1`/`p3` NDJSON acquisition fields import via `acquire --from works.ndjson` | #286 | status preserved character-for-character in `reason` |
| *(scope)* scan / attach, manifest mirror, `gc`, OSTI route, identity checks | #285, #286 | 893 tests at that point |
| *(scope)* TUI and `read_paper` onto artefacts, legacy writes stop | #290 | the three columns are **deleted**, so a stale reader is a compile error |

## Five unwritten tables, found by opening the same door five times

Every slice opened by finding a migration-013 table or column with no writer anywhere in the workspace: `publisher` and `next_attempt_at` (blocking `acquire --resume`), `paper_identity_checks`, `acquisition_leases`, and the `bad_magic` / `too_large` attempt outcomes. Worth a deliberate sweep for the remainder rather than waiting for the next slice to trip over one.

## Three bugs found by measurement rather than review

- **7 of 27 "SI" files were HTML landing pages.** A magic-byte *table* would have let them through, since SI is unbounded — so an HTML page is now refused under every non-HTML declared format. `too_large` also got a writer, and is keyed on **kind** so a big workbook cannot be filed as a figure to get under a smaller cap.
- **A gc window no lock can close.** `store_blob` writes the file outside the transaction, and short-circuits when the digest already exists — so a fetch writes no file and commits its reference afterwards. gc committing in that window removed both row and file. A 24 h grace period does close it; nothing else could.
- **A `/var` → `/private/var` path comparison** that was a real product defect twice (the importer refused every legitimate file on macOS) and a test-only failure the third time. A raw path comparison is a test that only runs on one platform.

## Deliberately not closed here

- **#287** — the reader's provenance display, TUI chrome, and `read_paper`'s MCP return wording. The buildable part landed: `Provenance` + `UntrustedText` whose `Display` *is* the neutralised form, applied at every render boundary that shows publisher text today. A publisher `/Title` or `citation_title` can no longer carry terminal escapes, OSC 8 hyperlinks, bidi overrides or zero-width characters into a rendered string.
- **#288** — `too_large` is enforced on local files but **not on a fetched body**, so an HTTP route can still write a 2 GB blob.
- **#289** — `scitadel download <doi>` records no artefact, so it no longer marks a paper downloaded. It never did record one; the legacy columns were the only thing it wrote.
- **#260** — the 52-DOI OA-publisher half, which needs #254's index routes. Its papers now report `pending`/`error` rather than a no-access verdict, so the human list stopped lying even before that lands.

