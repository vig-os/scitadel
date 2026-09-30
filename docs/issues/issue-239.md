---
type: issue
state: open
created: 2026-09-29T08:27:23Z
updated: 2026-09-29T08:27:23Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/239
comments: 0
labels: bug, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:25.699Z
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

