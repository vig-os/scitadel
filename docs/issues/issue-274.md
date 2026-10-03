---
type: issue
state: open
created: 2026-10-02T23:19:54Z
updated: 2026-10-02T23:19:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/274
comments: 0
labels: bug, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:41.759Z
---

# [Issue 274]: [ci(vhs): the ffmpeg+ttyd install step is cancelled on a 10-minute timer, failing unrelated PRs](https://github.com/vig-os/scitadel/issues/274)

## Problem

The VHS workflow's shard jobs die at the `Install ffmpeg + ttyd` step after ~10m18s, with every later step `skipped` and the job reported as `failure`. It is not a test failure — the tapes never run.

Seen on PRs that do not touch the TUI or CLI at all, including ones whose only change was a Rust source file:

| PR | Diff | Failing step |
|---|---|---|
| #270 | `.github/workflows/rust-ci.yml`, one crate's `Cargo.toml` | `Install ffmpeg + ttyd` (cancelled) |
| #272 | `scitadel-core`, `scitadel-export` | `Install ffmpeg + ttyd` (cancelled) |
| #264 | `scitadel-core`, `scitadel-cli` (one line) | `Install ffmpeg + ttyd` (cancelled) |

It is not specific to any branch. `vhs.yml` runs show the same `cancelled` outcome on `dev` (2026-09-29) and on `main`.

## Why it matters

A flaky check that fails on unrelated PRs trains people to re-run CI until green, which is exactly how a real failure gets missed. It also blocks merge, since the job is required.

## Open question — this may be a fix I should apply rather than file

The consistent ~10m duration and the `cancelled` conclusion (rather than `failure`) suggest a timeout rather than an apt error. The job sets `timeout-minutes` at the job level; if the *step* has no timeout of its own, the apt call is being killed by something external. I have not yet established the cause, so I am filing rather than guessing at a fix.

## Proposal

1. Find the actual cause: capture the step's output rather than the summarised conclusion. `gh run view --job <id> --log` returned nothing for this step, which is itself a clue — the log is not being flushed before cancellation.
2. Whatever it is, stop the shard jobs from failing on it. A tape shard that could not *set up* should report as `neutral`/skipped with a loud warning, not as a red X that blocks an unrelated PR.
3. Add the ffmpeg/ttyd install to a cached toolchain step, or use a pinned container image, so the 10 minutes is not paid on every job.

## Acceptance

- [ ] A PR that changes no TUI/CLI code cannot fail a tape shard.
- [ ] The setup step either succeeds reliably or reports itself as unable to run, with the reason visible.

Refs: #239 (this workflow was audited for a different dead-job bug while fixing that; this is the remaining flakiness in the same file)
