---
type: issue
state: closed
created: 2026-09-28T12:10:23Z
updated: 2026-09-28T12:58:38Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/225
comments: 2
labels: chore, effort:small, priority:medium, area:workspace, semver:patch
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:26.060Z
---

# [Issue 225]: [chore(repo): upgrade devkit scaffold 1.6.0 -> 1.17.0](https://github.com/vig-os/scitadel/issues/225)

## Context

`.vig-os` pinned `DEVKIT_VERSION=1.6.0`; devkit released **1.17.0** on 2026-09-28,
eleven minor releases later. No breaking change in that range affects this repo
(the one `Breaking` entry, devkit#1421, removes the `devc-upgrade` recipe, a
devcontainer-mode convenience this `direnv`-mode repo never used).

## Scope

- Regenerate the scaffold at 1.17.0. Adds `abandon-release.yml` (draft-release
  rejection lane), `prepare-hotfix.yml` (gitflow hotfix lane) and
  `.github/actionlint.yaml`; retires the `renovate-changelog` build/commit pair
  that release-time changelog synthesis replaced upstream.
- Bump the pinned `vigos` flake input **in lockstep with `DEVKIT_VERSION`**.
  `flake.nix` is a preserved file the scaffold cannot touch, and it supplies
  `vig-utils` (the release train's changelog scripts) and `pymarkdown`. Left
  behind, CI runs the 1.6.0 toolchain against the 1.17.0 scaffold.
- Set the knobs added since 1.6.0 deliberately: `DEVKIT_LANGUAGES=rust` (seeded
  from `Cargo.toml` detection), `DEVKIT_LICENSE=none` (this workspace is
  `MIT OR Apache-2.0` and carries its own `LICENSE-MIT`/`LICENSE-APACHE`, so
  devkit should not manage an Apache-only `LICENSE`). All others keep defaults.
- Fold the `shellcheck` hook off the `shellcheck-py` repo onto the
  flake-resolved `language: system` form, and declare `shellcheck` + `actionlint`
  in the dev shell for the newly inserted hooks.
- Remove four retired scaffold paths orphaned by the 0.3.3 -> 1.6.0 upgrade.

## Acceptance Criteria

- [ ] `DEVKIT_VERSION` and the `vigos` flake input both read `1.17.0`
- [ ] `prek run --all-files` green
- [ ] `just test` runs the real suite (not a vacuous pass)
- [ ] CI green, **including `Scaffold Drift`**

## Notes

`devkit-upgrade.yml` could not be used for this bump: the legacy
`DEVKIT_UPGRADE_APP_ID` org secret is not scoped to this repo, and the 1.6.0
workflow reads only that key while 1.17.0 reads `DEVKIT_UPGRADE_APP_CLIENT_ID`
(which *is* granted here) — so the repo could not upgrade itself to the version
that fixes its own upgrade path. Done via `install.sh --force` instead; future
bumps can use the workflow. This likely resolves part (a) of #209.

## Out of scope

Adopting `lib.mkRustProject`; the pre-existing `just lint` failure (local nix
rustc 1.95 vs an `#[allow]` for a clippy 1.98 lint); ruleset required-check set.
---

# [Comment #1]() by [c-vigo]()

_Posted on September 28, 2026 at 12:27 PM_

Shipped in #226 (squashed as `582c4a6` on `dev`).

`DEVKIT_VERSION=1.17.0` and the pinned `vigos` flake input moved together, so the
dev shell and CI now run the 1.17.0 toolchain. All acceptance criteria met:
`prek run --all-files` 20/20, `just test` 517 tests across 23 binaries, and CI
green on the PR including **`Scaffold Drift`** — the proof the regeneration was
complete.

Notable deviation from the plan: `devkit-upgrade.yml` could not perform the bump.
The legacy `DEVKIT_UPGRADE_APP_ID` org secret is not scoped to this repo, and the
1.6.0 workflow read only that key while 1.17.0 reads
`DEVKIT_UPGRADE_APP_CLIENT_ID` (which *is* granted here) — the repo could not
upgrade itself to the version that fixes its own upgrade path. Done via
`install.sh --force`; future bumps can use the workflow, which also likely
resolves part (a) of #209.

Follow-ups: #227 (ruleset required-check set), #228 (devkit governance hooks +
`core.hooksPath`), #229 (pre-existing `just lint` / Rust pin divergence).

---

# [Comment #2]() by [c-vigo]()

_Posted on September 28, 2026 at 12:58 PM_

Filed **vig-os/devkit#1752** for the flake-pin half of this upgrade.

The manual second step this upgrade needed — bumping `flake.nix`'s `vigos` pin
alongside `DEVKIT_VERSION` — recurs on **every** future bump, because
`install.sh` deliberately never rewrites a pinned input and `flake.nix` is a
`PRESERVE_FILE`. Nothing currently catches a missed bump: `scaffold-drift` cannot
see a preserved file, and `devkit-staleness` only compares `DEVKIT_VERSION` against
devkit's latest release.

The concrete cost, from this very upgrade: `prepare-changelog` gained a `seed`
subcommand between 1.6.0 and 1.17.0, and the 1.17.0 `release-core.yml` calls it —
so scaffold-at-1.17.0 with flake-at-1.6.0 fails *mid-release*.

devkit#1752 proposes a `DEVKIT_FLAKE_PIN_ADVANCE` knob (opt-in, default
byte-identical to today) plus a CI guard in the managed `ci.yml` so the mismatch
fails a PR rather than being warned about once in a log. Both halves are
devkit-managed, so no local check is needed here once it lands.

**Until then: every devkit bump on this repo is a two-step** — advance
`DEVKIT_VERSION` *and* `flake.nix`'s `vigos` ref, then `nix flake update vigos`.
The installer does warn loudly at scaffold time; read that log.

