---
type: issue
state: open
created: 2026-09-28T12:13:25Z
updated: 2026-09-28T23:16:45Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/227
comments: 1
labels: chore, effort:small, priority:medium, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:25.535Z
---

# [Issue 227]: [chore(ci): require the devkit ci.yml checks in the main and dev rulesets](https://github.com/vig-os/scitadel/issues/227)

## Context

Both branch rulesets gate merges on `rust-ci.yml`'s job names only:

| Ruleset | Required checks |
|---|---|
| `main protection` (15240992) | `Lint`, `Test (ubuntu-latest)`, `Test (macos-latest)` |
| `dev protection` (15240993) | `Lint`, `Test (ubuntu-latest)` |

Devkit's scaffolded `ci.yml` contributes `Lint & Format`, `Tests`,
`Commit Messages`, `Scaffold Drift` and the `CI Summary` aggregate gate. **None
of them are required**, so all five can fail and a PR still merges.

`Scaffold Drift` is the costly one to leave ungated: it re-runs the pinned
devkit version's scaffold over the checkout and fails when a managed file was
hand-edited or `DEVKIT_VERSION` moved without a re-scaffold. Unrequired, scaffold
drift can land silently and is then discovered at the next upgrade.

Pre-dates #225 (the 1.17.0 bump) — deliberately kept out of that PR because it
changes merge gating for everyone.

## Proposal

Require on **both** rulesets:

- `CI Summary` — devkit's aggregate gate, which already depends on the rest
- `Scaffold Drift`
- `Commit Messages`

Keep `Test (macos-latest)` on `main`: devkit's `Tests` job runs Linux only, so
the cross-OS matrix in `rust-ci.yml` is not redundant.

## Open question

`rust-ci.yml`'s `Lint` and devkit's `Lint & Format` overlap — `Lint` runs
`cargo fmt --check` + `cargo clippy -D warnings`, `Lint & Format` runs
`just precommit` (the prek suite, which includes `cargo fmt --check`). Decide
whether to keep both, or drop `rust-ci.yml`'s `Lint` and move clippy into the
`precommit` suite. Note the clippy-version mismatch in the linked issue affects
that choice.

## Acceptance Criteria

- [ ] Each new context has reported at least once on a PR (a required check that
      never reports blocks all merges)
- [ ] `gh api repos/vig-os/scitadel/rulesets/15240992` and `/15240993` list the
      agreed set
- [ ] A PR with deliberate scaffold drift is blocked

Refs: #225
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 11:16 PM_

Tracking note: scitadel's rulesets and merge settings are declared in vig-os/org-config (otterdog), so this lands there, not as a repo-side change. **vig-os/org-config#301** already covers it:
- `CI Summary` required on `dev`, `release/*` and `main`. It aggregates `Commit Messages` and `Scaffold Drift`, so those two gate as well.
- The `commit-action-bot` bypass on `dev` and `release/*`.
- Merge commits, which `promote-release` needs for its `gh pr merge --merge`.

For reference, `CI Summary`, `Commit Messages` and `Scaffold Drift` all reported and passed on the App-authored release PR (#222) and sync PR (#223), as well as on human PRs. Requiring them won't strand the automated flows.

One thing worth knowing until #301 applies: `main` is squash-only again (the 09-24 apply reverted the merge-commit toggle), so a `promote-release` run today would fail at `gh pr merge --merge`. Don't cut the next release before #301 is applied.

