# Releasing

Releases run on the **vigOS devkit release train**. The generic, managed flow —
`prepare-release.yml` → `release.yml` → `promote-release.yml`, the RC train, the
rollback semantics and the extension seams — is documented once in
[`DOWNSTREAM_RELEASE.md`](./DOWNSTREAM_RELEASE.md); this page only records what
is specific to scitadel.

Until 0.6.0 releases were cut by release-please. That system was retired in #207.

## Flow

1. Land conventional commits on `dev` (the gitflow integration branch).
2. Keep the `## Unreleased` section of [`CHANGELOG.md`](../CHANGELOG.md) current
   as you go — the train reads it, it is not generated from commit messages any
   more.
3. Dispatch **`prepare-release.yml`** with the target version. It freezes the
   changelog, cuts `release/X.Y.Z`, runs the prepare seam (below) and opens the
   draft release PR to `main`.
4. Dispatch **`release.yml`** (`release_kind: candidate`) as many times as
   needed to publish `X.Y.Z-rcN` tags, then once with `release_kind: final` to
   publish tag `X.Y.Z` and its **draft** GitHub Release.
5. Wait for **`binaries.yml`** to finish attaching the platform tarballs to that
   draft (see below — assets cannot be added after promotion).
6. Dispatch **`promote-release.yml`**. It publishes the Release and merges
   `release/X.Y.Z` into `main`.
7. Publishing the Release triggers **`publish-crates.yml`**, which pushes all
   eight crates to crates.io in dependency order.

## Versioning model

Single shared version across the workspace. Each `crates/scitadel-*/Cargo.toml`
carries an explicit `version = "x.y.z"` (not `version.workspace = true`), and the
crates depend on each other with explicit `version = "…"` requirements. There is
no `[workspace.package].version`.

The devkit train owns `CHANGELOG.md`, the tag and the GitHub Release — it does
**not** touch language manifests. Keeping the manifests in lockstep is this
repo's job, and it is done by
[`prepare-release-extension.yml`](../.github/workflows/prepare-release-extension.yml):
when the release branch is cut it runs `cargo set-version --workspace X.Y.Z`
(cargo-edit), which rewrites every member version, every inter-crate requirement
and `Cargo.lock`, then commits the result onto `release/X.Y.Z` so it is part of
the release PR diff. This replaces release-please's `cargo-workspace` plugin.

Tags are **unprefixed** `X.Y.Z` (`DEVKIT_TAG_PREFIX` is empty in
[`.vig-os`](../.vig-os)), matching the tags release-please emitted. The stray
early `v0.5.0` / `v0.6.0` tags are historical and harmless.

## Release-time workflows owned by this repo

| Workflow | Trigger | What it does |
| --- | --- | --- |
| `prepare-release-extension.yml` | called by `prepare-release.yml` | `cargo set-version --workspace`, committed to the release branch |
| `release-extension.yml` | called by `release.yml` before the tag is cut | crates.io publishability gate at the finalized SHA; failing it rolls the release back |
| `binaries.yml` | final tag push, or manual dispatch | builds the three platform tarballs + SHA256 sums, uploads them into the still-draft Release |
| `publish-crates.yml` | `release: published`, or manual dispatch (plus an always-on PR dry-run) | publishes the crates to crates.io |

Ordering matters in exactly one place: **`binaries.yml` must finish before
`promote-release.yml` runs.** GitHub's immutable releases refuse asset uploads
once a Release is published, so the tarballs have to land while it is still a
draft. `binaries.yml` cannot move into the read-only `release-extension.yml`
seam, because that seam runs before the tag and the Release even exist.

## One-time setup (repo secrets and variables)

| Secret / variable | Why |
| --- | --- |
| `RELEASE_APP_CLIENT_ID`, `RELEASE_APP_PRIVATE_KEY` | devkit Release App — tags, Releases, promote. See `DOWNSTREAM_RELEASE.md`. |
| `COMMIT_APP_CLIENT_ID`, `COMMIT_APP_PRIVATE_KEY` | devkit Commit App — the changelog freeze and the workspace version-bump commit. |
| `CARGO_REGISTRY_TOKEN` (environment `crates-io`) | crates.io API token used by `publish-crates.yml`. |

The retired release-please credentials (`RELEASE_PLEASE_TOKEN`,
`RELEASE_BOT_APP_ID`, `RELEASE_BOT_PRIVATE_KEY`) can be deleted.

## First release on the train

The first release after migrating has a one-time sharp edge: `promote-release.yml`
is not yet dispatchable, because GitHub only registers a `workflow_dispatch`
workflow that exists on the default branch, and the thing that puts it on `main`
is the promote merge itself. Follow the **first-release manual promote runbook**
in the devkit
[`MIGRATION.md`](https://github.com/vig-os/devkit/blob/main/docs/MIGRATION.md)
— run it through to the end in one go, it cannot be resumed by the workflow.
Every subsequent release promotes normally.

## Plugin version pin

The Claude Code plugin fetches a pinned release binary. After a release, bump
`plugins/scitadel/bin/VERSION` and `plugins/scitadel/.claude-plugin/plugin.json`
`version` to the new tag so the plugin installs the matching binary.
