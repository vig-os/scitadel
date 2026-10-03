---
type: issue
state: closed
created: 2026-08-06T12:48:22Z
updated: 2026-09-24T12:22:37Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/207
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:55.204Z
---

# [Issue 207]: [chore(repo): upgrade devkit scaffold 0.3.3 -> 1.6.0 (direnv, keep release-please)](https://github.com/vig-os/scitadel/issues/207)

## Context

This repo is pinned to the legacy devkit (devcontainer) scaffold **0.3.3** via the legacy `DEVCONTAINER_VERSION` key — ~13 releases behind current **1.6.0** and predating the `devkit-upgrade.yml` self-upgrade workflow. Manual migration per devkit `docs/MIGRATION.md` ("Upgrading an existing 0.3.x consumer").

An `install.sh --preview` dry-run (1.6.0, `--mode direnv --workflow gitflow`) is clean: 25 overwrite / 13 preserve / 1 delete / 55 add.

## Decisions

- **Migrate the release system to the devkit release train** — retire release-please + its config. Crate publishing and binary attachment move into the consumer-owned `release-extension.yml` seam.
- Delivery mode `direnv` (repo-owned `flake.nix`/`.envrc` are preserved by the scaffold), workflow `gitflow` (dev+main exist), prune the stale 0.3.3-era `.devcontainer/`.

## Scope

- `install.sh --version 1.6.0 --force --mode direnv --workflow gitflow --prune-devcontainer` (full scaffold incl. release train)
- Retire release-please: remove `release-please.yml`, `release-please-config.json`, `.release-please-manifest.json`
- Re-home `publish-crates.yml` / `binaries.yml` logic onto the devkit train (release-extension seam); match the existing tag scheme via `DEVKIT_TAG_PREFIX`
- Work the MIGRATION.md 0.3.x checklist: `pre-commit` → `prek` renames in preserved files, justfile append-block review, `.pre-commit-config.yaml` template-diff review, project-name re-derivation check
- Verify `prek` is resolvable in the repo dev shell (repo-owned flake does not import devkit's `mkProjectShell`); extend the devShell minimally if not
- Remove leftover Dependabot config (Renovate is the devkit path; open dependabot branches can be closed separately)
- Full hook suite green locally before PR

## Out of scope

Stale branch cleanup (`feature/wu*`, `rust-rewrite`, `release-please--branches--*`).

## Notes

First release on the devkit train needs the one-time manual-promote runbook (MIGRATION.md). Lars will be tagged on the PR for review of the release-system migration.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 24, 2026 at 12:22 PM_

Shipped in [0.8.0](https://github.com/vig-os/scitadel/releases/tag/0.8.0), the first release cut with the devkit train (prepare → release → promote). Follow-ups landed along the way:
- #214: registered the train workflows on main
- #221: explicit `FILE_PATHS` for the version-bump commit
- #224: crates.io trusted publishing, after the old token expired

