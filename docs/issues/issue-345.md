---
type: issue
state: open
created: 2026-11-09T05:59:40Z
updated: 2026-11-09T05:59:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/345
comments: 0
labels: bug, effort:medium, priority:medium, semver:patch
assignees: none
milestone: 0.10.0
projects: none
parent: none
children: none
synced: 2026-11-09T08:50:47.797Z
---

# [Issue 345]: [fix(db): one normalised Doi type — case-insensitive dedupe on every write path, indexed lookup, merge migration](https://github.com/vig-os/scitadel/issues/345)

## Problem

DOIs are case-insensitive (DOI Handbook §2.4), but scitadel normalises them inconsistently:
- PR #339 made the DOI lookup case-insensitive with `unicode_lower(doi)`. That expression can't use `idx_papers_doi`, so every lookup scans the whole `papers` table.
- The write-side duplicate checks are still case-sensitive (`crates/scitadel-db/src/sqlite/papers.rs:90,112,202,386,430`). The same work imported once as `10.1000/ABC` and once as `10.1000/abc` becomes two rows.

Found by the re-review of #339.

## Proposal

- Have one `Doi` newtype in `scitadel-core` that normalises once at construction: trim, strip `https://doi.org/` and `doi:` prefixes, and lowercase. Every adapter, import and write path builds a `Doi`, never a raw `String`.
- Add a migration that backfills the normalised value into a new indexed column (or rewrites `doi` in place), merges any duplicate rows it finds (deterministically, keeping the older row and reattaching artefacts, annotations and tags), and reports what it merged.
- Make lookups exact matches on the normalised, indexed column.

## Acceptance

- [ ] Importing the same DOI in mixed case yields one row, tested for each write path at the lines above.
- [ ] The migration merges existing duplicates and keeps all child rows, tested on a fixture DB.
- [ ] The DOI lookup uses the index; check with `EXPLAIN QUERY PLAN` in a test.

Refs #339, #289.

