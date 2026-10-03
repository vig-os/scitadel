---
type: issue
state: closed
created: 2026-09-29T08:27:23Z
updated: 2026-10-02T23:19:04Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/239
comments: 1
labels: bug, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:49.928Z
---

# [Issue 239]: [[BUG] rust-ci.yml contract-tests job never runs: guarded on schedule, but the workflow has no schedule trigger](https://github.com/vig-os/scitadel/issues/239)

## Description

The `contract-tests` job in `.github/workflows/rust-ci.yml` (default branch `main`) can never run. It is guarded on a `schedule` event, but the workflow declares no `schedule` trigger:

```yaml
on:  # yamllint disable-line rule:truthy
  push:
    branches: [main, dev, "rust-rewrite*"]
  pull_request:
    branches: [main, dev]
```

```yaml
  contract-tests:
    name: Contract Tests
    runs-on: ubuntu-latest
    if: github.event_name == 'schedule'
    steps:
      ...
      - run: cargo test --workspace --features contract-tests
```

On every `push` / `pull_request` run the job is skipped, so `cargo test --features contract-tests` is not exercised anywhere in CI. No other workflow in the repo runs it either.

## Expected Behavior

Either the contract tests run on a schedule, or the workflow does not carry a job that cannot fire.

## Fix

One of:

- add a `schedule:` (cron) trigger to `rust-ci.yml`, so the guard is satisfied as intended; or
- remove the job (or its guard) if scheduled contract tests are no longer wanted.

## Context

Noticed during the org-wide merge-protection audit, vig-os/org-config#294.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 2, 2026 at 11:19 PM_

Fixed by #270 (merged into `dev`).

Took the issue's second option — removed rather than scheduled. Adding `schedule:` would not have helped: `cargo test --workspace` and `cargo test --workspace --features contract-tests` both run **552 tests**, because the `contract-tests` feature was declared empty and nothing was gated behind it. Scheduling it would have produced a weekly green tick and zero coverage.

Audited every `github.event_name ==` guard against its workflow's `on:` block; this was the only unreachable one.

The unimplemented deliverable it was standing in for (DES-002 E4, live API contract tests) is now tracked on its own as #268.

