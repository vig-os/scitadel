---
type: issue
state: open
created: 2026-10-05T10:05:40Z
updated: 2026-10-05T10:05:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/291
comments: 0
labels: bug, priority:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-06T08:17:51.953Z
---

# [Issue 291]: [fix(acquire): eight artefact columns have no writer, and the manifest mirror reads four of them — so it reports nulls as if they were absent](https://github.com/vig-os/scitadel/issues/291)

## What prompted this

Five slices in a row (#284, #285, #286, #290) each opened by finding a migration-013 table or column with **no writer anywhere in the workspace**. So I audited all 9 tables rather than waiting for the sixth.

## The gap

`ArtefactWrite` (`crates/scitadel-db/src/sqlite/artefacts.rs:156`) is the only writer for `artefacts`, and it has no field for **eight** of migration 013's columns:

| column | read by | owns |
|---|---|---|
| `license_url` | **manifest mirror** | #254 (Crossref licences) |
| `license_content_version` | **manifest mirror** | #254 |
| `license_start` | **manifest mirror** | #254 |
| `license_source` | **manifest mirror** | #254 |
| `tdm_policy_ref` | nothing | #258 (tier-2 TDM) |
| `auth_context` | nothing | #258 |
| `retain_until` | nothing | #258 (retention for TDM-sourced material) |
| `derived_from` | nothing | unassigned — self-referential FK, presumably for SI/table/figure provenance |

`tdm_authorisations` is worse: only `publisher` has a writer. `basis`, `policy_ref`, `allows_bulk_session`, `max_works_per_day`, `granted_by`, `expires_at` are all unwritten, so #258 has no substrate to build on.

## Why this is more than an inventory

**The manifest mirror reads the four licence columns.** `manifest.rs:168-171` selects them and `214-217` emits them. Since nothing writes them, the generated `papers/<stem>/manifest.json` reports `null` for the licence of every artefact — forever, and indistinguishable from "we looked and there is no licence".

That is precisely the failure this project keeps fixing elsewhere: a report asserting absence where nothing was established. `artefacts.rs:773` even carries a comment saying the reader *must* select `license_url`, "because the reader did not select it is worse than no document" — and the writer that would populate it does not exist. The comment is currently aspirational.

`access_basis` is the one licence-adjacent column that *is* written, and it is derived from `RouteId::access_basis()` — a static classification of the route, not the publisher's actual licence. So the artefact does record *why* we believe access is lawful, but not *which licence*, which is the part a reader or a downstream tool needs.

## Proposal

1. **Do not add writers speculatively.** Each column should arrive with the slice that has real values for it, or not at all. An unwritten column is honest; a writer that always writes `NULL` is not.
2. **Make the manifest's silence explicit until then.** A mirror that emits `null` for a field nothing can populate should either omit the key or carry a note saying it is not recorded, rather than looking like a known-empty value. That is a small, independent fix and it removes the misleading output today.
3. **Fold the audit into CI** rather than leaving it to be redone. A cheap check — every column in an ADR-013 table is either written, read-and-documented-as-unwritten, or listed in an allowlist with a tracking issue — would stop the sixth slice opening the same way.
4. **Assign `derived_from`.** It is a self-referential FK with no owner. Either it is how SI/table/figure artefacts record which full text they were extracted from (#257 S5), in which case say so there, or it is unused and should be dropped in a migration with the ADR amended.

## Acceptance

- [ ] Every migration-013 column is written, or documented as not-yet-recorded with the issue that owns it.
- [ ] The manifest mirror does not present an unwritten column as a known-empty value.
- [ ] `tdm_authorisations` has its substrate before #258 starts.
- [ ] A CI check fails when a new migration column is added with neither a writer nor an allowlist entry.

## Note on the sweep method

The first pass produced 20 false positives, because `write_artefacts_in` builds its column list as a runtime `{columns}` variable — a regex cannot see it. The list above was confirmed against the `ArtefactWrite` struct by hand. Any automated version of this check needs to read the write structs, not the SQL strings, or it will cry wolf.

Refs: #253, #254, #258, ADR-007 §1.
