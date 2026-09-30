---
type: issue
state: open
created: 2026-09-30T00:11:53Z
updated: 2026-09-30T00:11:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/250
comments: 0
labels: bug, effort:small, priority:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:22.469Z
---

# [Issue 250]: [fix(db): run_migrations is not transactional or locked — concurrent starts double-apply, failures leave partial schema](https://github.com/vig-os/scitadel/issues/250)

## Problem
`crates/scitadel-db/src/sqlite/migrations.rs`, `run_migrations` (L34), reads `schema_version` and then `execute_batch`es each pending migration (L57) with **no transaction and no lock**.
- **Concurrent starts:** the CLI, the MCP server and the TUI can open the same DB at once. Two processes starting together can both see version N and both run N+1. `ALTER TABLE … ADD COLUMN` then fails on the second run, or data statements run twice.
- **Mid-migration failure:** a multi-statement migration that fails halfway leaves a partial schema with no `schema_version` row, and the next start re-runs the half-applied migration.

This becomes important with #247, where migration 013 also backfills data (legacy downloads into artefacts).

## Fix
- Wrap each migration in `BEGIN IMMEDIATE`, re-check `schema_version` inside the transaction, apply, insert the version, then `COMMIT`.
- Rust-side data backfills run inside the same guarded step and are idempotent (`INSERT OR IGNORE` on a natural key).
- Tests: two threads with two connections run migrations on the same temp file, and each migration applies exactly once. A deliberately failing migration leaves the schema at N.

Found in the #247 data-architecture review.

Refs: #247
