---
type: issue
state: open
created: 2026-10-04T21:41:14Z
updated: 2026-10-04T21:41:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/289
comments: 0
labels: bug, effort:small, priority:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T07:53:08.123Z
---

# [Issue 289]: [fix(cli): scitadel download &lt;doi&gt; records no artefact, so it no longer marks the paper downloaded](https://github.com/vig-os/scitadel/issues/289)

## Problem

S2e (#287) retired the legacy dual-write: `record_download` no longer writes `papers.local_path` / `download_status` / `last_attempt_at`, because ADR-007 §1 makes the database the source of truth and "have" is derived from `artefacts`.

`scitadel download <doi>` went through `PaperDownloader::download()` — the **non-recording** path. It never wrote an artefact, so before S2e the legacy columns were the only thing it ever wrote; now it records nothing at all. A user who runs it gets a downloaded file and an unchanged library, and the paper does not show as downloaded in the TUI.

Found by the S2e implementation while retiring the columns; not caused by it.

## The options, and which I'd pick

1. **Make `download()` record an artefact** — the verb then does what its name says and the library reflects reality. This is the real fix: the current state is a verb that fetches a file and tells the database nothing.
2. **Deprecate `scitadel download <doi>`** in favour of `scitadel acquire`, which is idempotent, paced, identity-checked and records. Fewer paths, one behaviour.

Either is defensible. (1) is a smaller change and stops surprising users; (2) is better long-term and avoids a second way to acquire a full text.

For what it is worth, (2) matches what the codebase has been converging on: every acquisition path now wants the lease, the pacer, the identity check and the `acquisition_state` write, and `download()` has none of them — it is the last fetch path that bypasses the ladder.

## Acceptance

- [ ] `scitadel download <doi>` either records an artefact or is documented as a raw fetch that does not, with the reason.
- [ ] No path writes a file into the library without a row naming it.

Refs: #253 (S2e), #287.
