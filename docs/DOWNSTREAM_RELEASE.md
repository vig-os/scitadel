<!-- Managed by vigOS devkit — regenerated on upgrade; local edits are lost. -->
<!-- Customize in justfile.project. Bugs / missing tools: https://github.com/vig-os/devkit/issues -->

# Downstream Release Workflows

This document is the **only** place that describes the release process for **consumer projects** that install workflows from `assets/workspace/`. The upstream devcontainer and smoke-test validation flow is documented in [`docs/RELEASE_CYCLE.md`](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md) and [`docs/CROSS_REPO_RELEASE_GATE.md`](https://github.com/vig-os/devkit/blob/main/docs/CROSS_REPO_RELEASE_GATE.md).

## Overview

The downstream template uses a split release architecture:

- `prepare-release.yml` (`workflow_dispatch`) prepares `release/X.Y.Z`
- `release.yml` (`workflow_dispatch`) orchestrates:
  - `release-core.yml` (`workflow_call`)
  - `release-extension.yml` (`workflow_call`, project-owned)
  - `release-publish.yml` (`workflow_call`)
- `promote-release.yml` (`workflow_dispatch`) runs **after** a successful final `release.yml`: validates draft GitHub Release and release PR state, publishes the release, merges `release/X.Y.Z` to `main`, and best-effort cleans up remote git RC tags without a GitHub Release (no GHCR/cosign; see [Promote release (final)](#promote-release-final))

All files are deployed from `assets/workspace/` by `init-workspace.sh`.

On failure, the orchestrator runs a single consolidated rollback that resets the release branch (best-effort), does **not** delete tags (forward-fix policy), and opens a failure issue with forward-fix guidance.

## Release Modes

`release.yml` supports two release modes via `release_kind`:

- `candidate` (default): computes and publishes the next `X.Y.Z-rcN` git tag; optional workflow input **`create-release`** (default `false`) also creates a **draft** GitHub **pre-release**. Use optional `rc-number` to pin `N` when orchestrating from an upstream dispatch (see `docs/CROSS_REPO_RELEASE_GATE.md`). The smoke-test template passes `create-release=true` when it runs the workspace `release.yml` for a candidate.
- `final`: publishes `X.Y.Z`, finalizes `CHANGELOG.md` release date, runs `sync-issues`, and creates a **draft** GitHub Release (publish from the UI when review is complete; aligns with GitHub’s [immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases) and [draft-first guidance](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases#best-practices-for-publishing-immutable-releases))

Candidate mode keeps release branch content unchanged (no CHANGELOG date finalization). Final mode performs changelog finalization before publish.

## Immutable releases, tag rulesets, and forward-fix policy (downstream)

- **Candidate (`X.Y.Z-rcN`)**: By default only the git tag is created. With **`create-release: true`**, `release-publish.yml` creates a **draft** GitHub **pre-release** (`gh release create --draft --prerelease`). Promote-time validation uses `gh api .../releases/tags/<tag>` and inspects `.draft` to ensure the expected draft pre-release exists; see [Cross-repo gate](https://github.com/vig-os/devkit/blob/main/docs/CROSS_REPO_RELEASE_GATE.md) for upstream enforcement status. With **immutable releases** enabled, **publishing** a pre-release locks the **linked** tag and assets (see [upstream policy](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md#immutable-releases-tag-rulesets-and-forward-fix-policy)); iterate with a **new** RC tag.
- **Final (`X.Y.Z`)**: Automation creates a **draft** GitHub Release; **publishing** it (UI or `promote-release.yml`) applies immutable-release lock-in for the linked tag and assets when that setting is enabled. Enable **immutable releases** and **tag rulesets** on each consumer repository (and org policy) as needed; see [Preventing changes to your releases](https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/preventing-changes-to-your-releases).
- **Rollback**: The orchestrator resets the release branch and does **not** delete tags (forward-fix policy); recover with a new RC or a careful final retry per workflow logs.

## Promote release (final)

After final `release.yml` has pushed tag `X.Y.Z` and created a **draft** GitHub Release, run **`promote-release.yml`** (or `just promote-release X.Y.Z` from the devcontainer; dispatches on `release/X.Y.Z` by default) to:

1. **Validate** — semver, draft release for `X.Y.Z`, release PR not draft / approved / CI green
2. **Promote** — `gh release edit --draft=false`
3. **Merge** — merge `release/X.Y.Z` → `main` (triggers `sync-main-to-dev` under the gitflow model — see [Workflow models](#workflow-models))
4. **Cleanup** (best-effort, does not fail the workflow) — delete remote git tags matching `${VERSION}-rc*` that have **no** GitHub Release

**Upstream (`vig-os/devcontainer`) only:** Root `promote-release.yml` also prunes GHCR RC package versions via the org Packages API using **`GITHUB_TOKEN`** with **repo Admin** on the `devcontainer` package (one-time **Manage Actions access** grant). See [GitHub App Configuration](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md#github-app-configuration) and [Registry and cleanup tokens](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md#registry-and-cleanup-tokens-upstream) in `docs/RELEASE_CYCLE.md`.

This template does **not** implement upstream-only steps (GHCR `:latest`, cosign, cross-repo smoke-test gate). Projects that need registry or deploy promotion after merge should run separate automation or extend their `release-extension.yml` / own workflows; see [Extension Hook](#extension-hook).

## Workflow models

The whole release flow above is the same under either **workflow model** a
consumer selects with `DEVKIT_WORKFLOW` in `.vig-os` (`gitflow`, the default, or
`trunk` — [#1205](https://github.com/vig-os/devkit/issues/1205)): `release/X.Y.Z`
is still cut, driven through the RC train, finalized, and merged. The models
differ only in the base the release branch forks from and merges back to, which
is settled entirely at scaffold time (an anchored `dev -> main` render — see
[`docs/rfcs/ADR-workflow-model.md`](https://github.com/vig-os/devkit/blob/main/docs/rfcs/ADR-workflow-model.md)).
One release step is model-dependent:

- **`sync-main-to-dev.yml` runs only under `gitflow`.** The gitflow model keeps a
  long-lived `dev` integration branch, so a push to `main` (including a release
  merge) opens a PR syncing `main` back into `dev`. The `trunk` model has no
  `dev` branch — `feature`/`bugfix`/`chore` branches merge straight to `main` and
  `release/X.Y.Z` merges back into `main` — so `sync-main-to-dev.yml` is never
  scaffolded (copy-excluded, and pruned on a gitflow → trunk upgrade). The
  promote-time back-merge referenced above is therefore a no-op under `trunk`.

Consumer-facing opt-in, the destructive-switch preflight, and the orphan `dev`
cleanup caveat are documented in
[`docs/MIGRATION.md`](https://github.com/vig-os/devkit/blob/main/docs/MIGRATION.md#workflow-models); the branching topology in
[`docs/RELEASE_CYCLE.md`](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md#workflow-models).

## Workflow Interface

The orchestrator `release.yml` passes release context directly to the called reusable workflows:

- `.github/workflows/release-core.yml`
- `.github/workflows/release-extension.yml`
- `.github/workflows/release-publish.yml`

There is no separate contract-version handshake; compatibility is defined by the `workflow_call` input schema in each workflow file.

`promote-release.yml` is a standalone `workflow_dispatch` workflow (input: `version`); it does not call the reusable workflows above.

## Toolchain provisioning is mode-aware

Since [#991](https://github.com/vig-os/devkit/issues/991), the whole
release/automation set provisions its toolchain per `DEVKIT_MODE`
(`.vig-os`), following the conditional-`container:` pattern in
[`docs/rfcs/ADR-conditional-container-toolchain.md`](https://github.com/vig-os/devkit/blob/main/docs/rfcs/ADR-conditional-container-toolchain.md):

- Each `workflow_dispatch`/event-triggered workflow (`release.yml`,
  `prepare-release.yml`, `promote-release.yml`, `sync-issues.yml`,
  `renovate-changelog-build.yml`, `sync-main-to-dev.yml`) runs a leading
  **`resolve-toolchain`** job that reads `.vig-os` and emits `mode`, `image`, and
  `image-tag`. The `image` is the devcontainer image in the container modes
  (`devcontainer`/`both`) and an **explicit empty string** in the host modes
  (`direnv`/`bare`), which makes each downstream `container:` job run directly on
  the runner. `prepare-release.yml` runs the same composite **inline** in its host
  `validate` job and exposes the outputs to the `prepare` job.
- Every job then runs the **`setup-devkit-toolchain`** composite as its first
  step after checkout: it is a no-op-friendly preamble that exports the in-image
  env in the container modes, builds the repo's flake dev-shell in `direnv`, or
  `uv tool install`s the pinned host toolchain (incl. `vig-utils`) in `bare`.
- The orchestrator `release.yml` **resolves once** and threads the result into
  the reusable workflows via the `toolchain_mode`, `toolchain_image`, and
  `devkit_version` `workflow_call` inputs; `release-core.yml` /
  `release-publish.yml` do **not** re-resolve.

This is a toolchain-provisioning change only — the release **choreography** (step
logic, ordering, `workflow_call` inputs/outputs, and rollback semantics) is
unchanged across all modes. Host-mode runners already provide `git`, `gh`, and
`jq`; `just`, `uv`, `prek`, `retry`, and the `vig-utils` release scripts
(`prepare-changelog`, `renovate-changelog-pr`) come from the composite, so the
choreography's bare `run:` invocations are identical in every mode. In `bare`
mode the composite pins `vig-utils` to the `.vig-os` `DEVKIT_VERSION`
(`renovate-changelog-pr` in `renovate-changelog-build.yml`, `prepare-changelog`
in `prepare-release.yml` / `release-core.yml`); see
[`docs/MIGRATION.md`](https://github.com/vig-os/devkit/blob/main/docs/MIGRATION.md#bare-mode-vig-utils-release-console-scripts).

## Required App Secrets

Downstream repositories are expected to provide both app credentials:

- `COMMIT_APP_ID` (required by `vig-os/sync-issues-action` in `sync-issues.yml`)
- `COMMIT_APP_CLIENT_ID`
- `COMMIT_APP_PRIVATE_KEY`
- `RELEASE_APP_CLIENT_ID`
- `RELEASE_APP_PRIVATE_KEY`

Template behavior relies on explicit app-token generation for release operations:

- use **Commit App** token for protected branch/ref writes (`commit-action`, branch/tag mutation)
- use **Release App** token for release orchestration and PR/release API operations

`github.token` is intentionally not used as a fallback for these release write paths.

## Input Naming Convention

All `workflow_call` inputs use underscores (e.g. `release_kind`, `dry_run`, `git_user_name`). The orchestrator `release.yml` translates its own `workflow_dispatch` hyphenated inputs at each call site.

## Extension Hook

Project-specific release behavior belongs in `.github/workflows/release-extension.yml`.

Default template behavior is no-op. Projects can customize this workflow for tasks such as:

- package publishing
- container publishing
- signing and attestations
- release artifact upload

Extension contract inputs include both `release_kind` and `publish_version`, so custom logic can branch on candidate vs final behavior.

`release.yml` requires extension success before publish, so extension failures block release publication.

### Permission ceiling

A called reusable workflow can only **downgrade** the caller's `GITHUB_TOKEN` — it can never elevate it (issue [#1144](https://github.com/vig-os/devkit/issues/1144)). So the *maximum* token scope this seam can reach is set by the `extension` caller job in the managed `release.yml`, which grants:

| Scope | Level | For |
| --- | --- | --- |
| `contents` | `read` | check out the finalized commit |
| `packages` | `write` | container / package publishing (e.g. GHCR) |
| `id-token` | `write` | keyless cosign signing + provenance via OIDC |
| `attestations` | `write` | build provenance attestations (`actions/attest-build-provenance`) |

This is a **ceiling, not a grant**. The shipped default no-op declares `permissions: contents: read` and stays read-only. To publish, sign, or attest, declare the scopes your step needs **on your own job** (up to the ceiling) — e.g. a job that runs `actions/attest-build-provenance` sets `permissions: { id-token: write, attestations: write }`. Deny-by-default is preserved: no job gets a write token it did not ask for.

If an extension needs a scope **beyond** this ceiling (for example `contents: write` to push to a branch, which the read-only seam intentionally forbids), it belongs in a consumer-owned tag-push or post-release workflow that owns its own token grant — e.g. a workflow on `push: tags: 'v*.*.*'` with its own `permissions:` block — not in this seam.

## Prepare-Release Extension Hook

Project-specific **release-branch preparation** belongs in `.github/workflows/prepare-release-extension.yml` — the *mutating* counterpart to the read-only `release-extension.yml`. Default template behavior is no-op.

`prepare-release.yml` calls it as a reusable workflow **after** the `release/X.Y.Z` branch is created (and the changelog-freeze commit pushed) and **before** the draft PR to `main` is opened, so any commits a consumer's extension pushes to the fresh release branch appear in the PR diff from the start. Because a `workflow_call` workflow is a job, the prepare phase is split into jobs (`prepare` creates the branch, `extension` runs the hook, `open-pr` opens the draft PR).

Contract inputs:

- `version` — the release version being prepared (`X.Y.Z`)
- `release_branch` — the release branch just created (`release/X.Y.Z`)
- `branch_sha` — the post-freeze head SHA the release branch was created from
- `dry_run` — validate without making changes (extensions must honor it)
- `git_user_name`, `git_user_email` — the git identity `prepare-release.yml` carries

`prepare-release.yml` calls the hook with `secrets: inherit`, so an extension can mint the `COMMIT_APP` token to push to the write-protected release branch — the same bypass and identity the changelog-freeze commit already uses.

Semantics:

- **`dry_run: true`** ⇒ the default no-op prints its inputs and a consumer extension must not write. In the shipped `prepare-release.yml`, the whole prepare phase (including the `extension` job) is gated off on a dry run, so the hook only runs for real preparations.
- **Rollback** ⇒ an extension failure fails the prepare phase, and a single `rollback` job (which lists `extension` in `needs`) deletes the partial `release/X.Y.Z` branch and restores `CHANGELOG.md` on `dev`. No new rollback machinery is required: every commit the extension pushes lives on the release branch the rollback deletes.
- Anything the extension commits is ordinary release-branch history, re-validated by the rest of the pipeline (CI on the draft PR, RC candidates, finalize).

### Example: rebuild a committed build artifact (`vig-os/commit-action`)

An action-publishing repo must keep its committed `dist/index.js` fresh on every tagged commit. The prepare-time hook rebuilds it on the freshly cut release branch and commits it, so the release PR's `Dist Check` becomes pure verification:

```yaml
name: Prepare Release Extension

on:
  workflow_call:
    inputs:
      version:
        required: true
        type: string
      release_branch:
        required: true
        type: string
      branch_sha:
        required: true
        type: string
      dry_run:
        required: false
        default: false
        type: boolean
      git_user_name:
        required: false
        type: string
      git_user_email:
        required: false
        type: string

permissions:
  contents: read

jobs:
  rebuild-dist:
    name: Rebuild and Commit dist/
    runs-on: ubuntu-24.04
    if: ${{ inputs.dry_run != true }}
    steps:
      - name: Generate Commit App Token
        id: commit_app_token
        uses: actions/create-github-app-token@v3
        with:
          client-id: ${{ secrets.COMMIT_APP_CLIENT_ID }}
          private-key: ${{ secrets.COMMIT_APP_PRIVATE_KEY }}

      - name: Checkout release branch
        uses: actions/checkout@v5
        with:
          ref: ${{ inputs.release_branch }}

      - name: Build the action bundle
        run: |
          just sync
          just bundle

      - name: Commit dist/ if it changed
        if: ${{ hashFiles('dist/index.js') != '' }}
        uses: vig-os/commit-action@v0
        env:
          GH_TOKEN: ${{ steps.commit_app_token.outputs.token }}
          GITHUB_REPOSITORY: ${{ github.repository }}
          TARGET_BRANCH: refs/heads/${{ inputs.release_branch }}
          COMMIT_MESSAGE: |-
            chore: rebuild dist for release ${{ inputs.version }}
          FILE_PATHS: dist/index.js
```

## Cross-Repo Validation Gate

Cross-repository validation gate details are documented in `docs/CROSS_REPO_RELEASE_GATE.md`.

### Example: GHCR Publishing

The following shows how a downstream project could customize `release-extension.yml` to build and push a container image to GHCR:

```yaml
name: Release Extension

on:
  workflow_call:
    inputs:
      version:
        required: true
        type: string
      finalize_sha:
        required: true
        type: string
      release_date:
        required: true
        type: string
      release_kind:
        required: true
        type: string
      publish_version:
        required: true
        type: string
jobs:
  ghcr-publish:
    name: Publish Container Image
    runs-on: ubuntu-22.04
    permissions:
      contents: read
      packages: write
    steps:
      - name: Checkout finalized commit
        uses: actions/checkout@v4
        with:
          ref: ${{ inputs.finalize_sha }}

      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Build and push image
        uses: docker/build-push-action@v6
        with:
          context: .
          push: true
          tags: |
            ghcr.io/${{ github.repository }}:${{ inputs.publish_version }}
            ${{ inputs.release_kind == 'final' && format('ghcr.io/{0}:latest', github.repository) || '' }}
```

## Upgrade Path

1. Upgrade downstream devcontainer version (which redeploys `assets/workspace` templates).
2. Keep project-owned `release-extension.yml` (preserved on force upgrades).
3. Ensure project-owned `release-extension.yml` matches the current `workflow_call` inputs used by `release.yml`.
4. Run `prepare-release` / `release` in `--dry-run` mode to validate integration.

## Pinning and Drift

Release workflow logic is centralized in shipped local reusable workflows (`release-core.yml`, `release-publish.yml`) while extension logic remains project-owned (`release-extension.yml`).

This reduces drift in release safety checks while preserving downstream customization boundaries.
