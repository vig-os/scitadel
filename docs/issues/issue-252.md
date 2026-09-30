---
type: issue
state: open
created: 2026-09-30T00:26:03Z
updated: 2026-09-30T00:26:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/252
comments: 0
labels: feature, effort:large, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:22.088Z
---

# [Issue 252]: [feat(acquire): S1 — work/artefact model, persistent pacer, PacedClient, importers](https://github.com/vig-os/scitadel/issues/252)

Slice S1 of ADR-007 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`, §1, §3, §4, §6).

**Scope**
- Migration 013 (schema in §1), plus a post-commit, idempotent legacy backfill that *copies* legacy files into `blobs/`.
- A new `scitadel-http` crate: `PacedClient`, which disables redirect following and follows redirects itself, taking one permit per hop.
- A SQLite `Pacer`: `BEGIN IMMEDIATE`, a rolling 24 h window, failing closed on `SQLITE_BUSY`, and sleeping outside the transaction. Includes the bucket policy table and the defaults from §4.
- A new `scitadel-acquire` crate: the `Route` trait and ladder. `download_paper` is rewired onto it with **the same outputs as today**, dual-writing the legacy columns and `papers/<stem>`.
- A workspace clippy lint banning raw `reqwest` use, with per-file exemptions.
- `scitadel import-flat` for consumer-written layouts (raid).
- Both new crates join the publish order.

**Acceptance** (§6, S1 row)
- [ ] Running the migration and backfill twice changes nothing on the second run.
- [ ] A two-process test on one database file never exceeds a cap.
- [ ] A wiremock redirect (A→302→B) takes two permits.
- [ ] `download_paper` produces the same outputs as today; the existing tests pass unchanged.
- [ ] `import-flat` imports a fixture of raid's layout without loss (tables, figure refs, `imported_from`).

**Depends on:** #250 (transactional migrations).
Refs: #247, #230, #234
