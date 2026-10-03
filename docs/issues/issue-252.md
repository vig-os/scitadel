---
type: issue
state: closed
created: 2026-09-30T00:26:03Z
updated: 2026-10-03T00:34:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/252
comments: 1
labels: feature, effort:large, phase:plan
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:46.726Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on October 3, 2026 at 12:34 AM_

Slice S1 complete, merged as #273 (8 commits).

## Acceptance criteria

| Criterion | Status |
|---|---|
| migration 013 plus an idempotent backfill | done — `013_acquisition.sql`, 5 migration tests, 13 backfill tests |
| `scitadel-http` with `PacedClient` and the SQLite ledger | done — new crate (52 tests) + `sqlite/pacer.rs` (15 tests) |
| the clippy lint is on for the workspace | already was; verified clean on the pinned 1.98.1 toolchain |
| a two-process test shows the cap isn't exceeded | `two_processes_do_not_exceed_the_cap` |
| a wiremock redirect test (A→302→B) shows two permits | `a_redirect_chain_spends_one_request_per_hop_across_two_buckets` |
| `download_paper` on `Route` + `PacedClient`, same outputs, dual-write | done — `record_download` writes both in one transaction, 6 tests |
| `import-flat` imports a fixture of raid's layout losslessly | done — 17 tests + 4 CLI tests |

720 tests pass across 26 binaries, green on ubuntu and macos. `cargo fmt --check` clean. Clippy `-D warnings` clean workspace-wide.

## Three things worth carrying forward

**1. A cap the ADR stated that nothing could enforce.** §3 caps tier 4 at "≤ 50 works per publisher, **≤ 150 works total**". The per-publisher half is `BucketPolicy::work_cap`, but a policy is per-bucket by definition, so the total had no home — four publishers at 50 works each would have authorised 200. `SqlitePacer::with_total_caps` adds the missing cross-bucket count inside the same transaction.

**2. macOS CI caught a containment check that was refusing legitimate files.** `within_root` canonicalised one side and not the other. On macOS that refuses every real file, because `/var` is a symlink to `/private/var` — and the refusal looked exactly like a working security check. Both sides now resolve via the deepest existing ancestor, so a dangling reference is recorded and a genuine symlink escape is still refused. Two tests pin it, both verified to fail against the old logic.

**3. Subagents share a working tree.** Running two in one checkout lost work once (a `cargo fmt` reordered a file under another agent's edit). The second agent noticed, stopped rather than racing, and handed off. Subsequent work was serialised. Worth using separate worktrees next time.

## Deliberately not fixed here

- **#276** — the OpenAlex API key is still in a query string and so reaches error strings and the TUI. Pre-existing; the change was made no worse.
- **#275** — `doi.org` still has no pacing bucket, so S2's identity check will resolve every DOI at 5 s. No limit was invented.
- The importer lives in `scitadel-adapters`; ADR §3 puts importers in a future `scitadel-acquire` crate.
- The TUI and CLI still write the legacy columns after the fact, so a *failed* download is still recorded that way. S2 removes the second writer.

Refs: #252

