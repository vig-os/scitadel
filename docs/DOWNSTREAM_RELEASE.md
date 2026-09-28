<!-- Managed by vigOS devkit — regenerated on upgrade; local edits are lost. -->
<!-- Customize in justfile.project. Bugs / missing tools: https://github.com/vig-os/devkit/issues -->

# Downstream Release Workflows

This document is the **only** place that describes the release process for **consumer projects** that install workflows from `assets/workspace/`. The upstream devcontainer and smoke-test validation flow is documented in [`docs/RELEASE_CYCLE.md`](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md) and [`docs/CROSS_REPO_RELEASE_GATE.md`](https://github.com/vig-os/devkit/blob/main/docs/CROSS_REPO_RELEASE_GATE.md).

## Overview

The downstream template uses a split release architecture:

- `prepare-release.yml` (`workflow_dispatch`) prepares `release/X.Y.Z`
- `prepare-hotfix.yml` (`workflow_dispatch`, gitflow only) prepares `release/X.Y.Z` **from `main`** for a patch that must not carry `dev` — see [Hotfix lane](#hotfix-lane-gitflow-only)
- `release.yml` (`workflow_dispatch`) orchestrates:
  - `release-core.yml` (`workflow_call`)
  - `release-extension.yml` (`workflow_call`, project-owned)
  - `release-publish.yml` (`workflow_call`)
- `promote-release.yml` (`workflow_dispatch`) runs **after** a successful final `release.yml`: validates draft GitHub Release and release PR state, publishes the release, merges `release/X.Y.Z` to `main`, and best-effort cleans up remote git RC tags without a GitHub Release (no GHCR/cosign; see [Promote release (final)](#promote-release-final)). With floating tags enabled (`DEVKIT_FLOATING_TAGS`), `validate` also refuses a version below the highest published final release of the tag-prefix line, so `<prefix>X` / `<prefix>X.Y` never move backwards ([#1626](https://github.com/vig-os/devkit/issues/1626))

All files are deployed from `assets/workspace/` by `init-workspace.sh`.

On failure, the orchestrator runs a single consolidated rollback that reverts only the finalize commit(s) this run wrote — it no-ops when finalize never ran and refuses to touch a branch that moved during the run ([#1462](https://github.com/vig-os/devkit/issues/1462)) — does **not** delete tags (forward-fix policy), and opens a failure issue with forward-fix guidance.

## Release Modes

`release.yml` supports two release modes via `release_kind`:

- `candidate` (default): computes and publishes the next `X.Y.Z-rcN` git tag; optional workflow input **`create-release`** (default `false`) also creates a **draft** GitHub **pre-release**. Use optional `rc-number` to pin `N` when orchestrating from an upstream dispatch (see `docs/CROSS_REPO_RELEASE_GATE.md`). The smoke-test template passes `create-release=true` when it runs the workspace `release.yml` for a candidate.
- `final`: publishes `X.Y.Z`, finalizes `CHANGELOG.md` release date, runs `sync-issues`, and creates a **draft** GitHub Release (publish from the UI when review is complete; aligns with GitHub’s [immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases) and [draft-first guidance](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases#best-practices-for-publishing-immutable-releases))

Candidate mode keeps release branch content unchanged (no CHANGELOG date finalization). Final mode performs changelog finalization before publish.

### Bot changelog entries (release-time synthesis)

Changelog entries for bot PRs (Renovate dependency updates, lock file
maintenance, devkit adoption PRs) are **synthesized at release time**, not
committed into the bot PR branches ([vig-os/devkit#1423](https://github.com/vig-os/devkit/issues/1423)):
`synthesize-bot-changelog` enumerates the merged bot PRs since the last stable
release tag and regenerates a `#### Dependencies` block under `### Changed`,
coalesced to the **net delta per dependency** (every contributing PR is cited).
It runs twice — in `prepare-release.yml` before the changelog freeze, and in
`release-core.yml` (final kind only) before the date stamp — so a bot PR merged
into the release branch mid-train is picked up at finalize. Candidates stay
changelog-neutral. Preview the pending block anytime with
`just changelog-preview` (read-only). Bot PR branches never touch
`CHANGELOG.md`, so Renovate's own conflict-driven rebase works unassisted.

## Immutable releases, tag rulesets, and forward-fix policy (downstream)

- **Candidate (`X.Y.Z-rcN`)**: By default only the git tag is created. With **`create-release: true`**, `release-publish.yml` creates a **draft** GitHub **pre-release** (`gh release create --draft --prerelease`). Promote-time validation uses `gh api .../releases/tags/<tag>` and inspects `.draft` to ensure the expected draft pre-release exists; see [Cross-repo gate](https://github.com/vig-os/devkit/blob/main/docs/CROSS_REPO_RELEASE_GATE.md) for upstream enforcement status. With **immutable releases** enabled, **publishing** a pre-release locks the **linked** tag and assets (see [upstream policy](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md#immutable-releases-tag-rulesets-and-forward-fix-policy)); iterate with a **new** RC tag.
- **Final (`X.Y.Z`)**: Automation creates a **draft** GitHub Release; **publishing** it (UI or `promote-release.yml`) applies immutable-release lock-in for the linked tag and assets when that setting is enabled. Enable **immutable releases** and **tag rulesets** on each consumer repository (and org policy) as needed; see [Preventing changes to your releases](https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/preventing-changes-to-your-releases).
- **Rollback**: The orchestrator reverts only the finalize commit(s) the failed run wrote (never a wholesale branch reset; it refuses when the branch moved mid-run, [#1462](https://github.com/vig-os/devkit/issues/1462)) and does **not** delete tags (forward-fix policy); recover with a new RC or a careful final retry per workflow logs.

## Promote release (final)

After final `release.yml` has pushed tag `X.Y.Z` and created a **draft** GitHub Release, run **`promote-release.yml`** (or `just promote-release X.Y.Z` from the devcontainer; dispatches on `release/X.Y.Z` by default) to:

1. **Validate** — semver, draft release for `X.Y.Z`, release PR not draft / approved (when the base branch requires reviews) / CI green
2. **Promote** — `gh release edit --draft=false`
3. **Merge** — merge `release/X.Y.Z` → `main` (triggers `sync-main-to-dev` under the gitflow model — see [Workflow models](#workflow-models))
4. **Cleanup** (best-effort, does not fail the workflow) — delete remote git tags matching `${VERSION}-rc*` that have **no** GitHub Release

**Approve the release PR immediately before dispatching promote** ([#1504](https://github.com/vig-os/devkit/issues/1504)) — this is the release cycle's single human approval; `release.yml` collects none. It happens here, after finalize, because the final `release.yml` run's `finalize` job pushes to `release/X.Y.Z` (CHANGELOG date stamp plus the `sync-issues` commit), and on any repository with stale-review dismissal enabled a push dismisses existing approvals — so an earlier approval could never survive to promote, and any later push to the release branch dismisses this one again:

```bash
# Current state (REVIEW_REQUIRED until approved)
gh pr view <PR_NUMBER> --json reviewDecision

# Approve (must be a human account other than the PR author)
gh -R <owner>/<repo> pr review <PR_NUMBER> --approve
```

On a repository whose `main` ruleset requires **no** approving reviews (solo projects), promote's gates skip the approval assertion — explicitly, logged — and this step disappears ([#1506](https://github.com/vig-os/devkit/issues/1506)). Full reasoning and the upstream runbook: [`docs/RELEASE_CYCLE.md`](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md#phase-5-post-release-cleanup).

**Upstream (`vig-os/devcontainer`) only:** Root `promote-release.yml` also prunes GHCR RC package versions via the org Packages API using **`GITHUB_TOKEN`** with **repo Admin** on the `devcontainer` package (one-time **Manage Actions access** grant). See [GitHub App Configuration](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md#github-app-configuration) and [Registry and cleanup tokens](https://github.com/vig-os/devkit/blob/main/docs/RELEASE_CYCLE.md#registry-and-cleanup-tokens-upstream) in `docs/RELEASE_CYCLE.md`.

This template does **not** implement upstream-only steps (GHCR `:latest`, cosign, cross-repo smoke-test gate). Projects that need registry or deploy promotion after merge should run separate automation or extend their `release-extension.yml` / own workflows; see [Extension Hook](#extension-hook).

## Abandon release (draft-only rejection path)

To **reject** a finalized-but-unpublished release instead of promoting it, run **`abandon-release.yml`** (or `just abandon-release X.Y.Z`; dispatches on `dev` by default — the release branch is about to be deleted, so it cannot be the dispatch ref; under the trunk model pass the ref explicitly: `just abandon-release X.Y.Z main`). As the Release App (tag-ruleset bypass, the same machinery as promote's RC prune) it deletes the **draft** GitHub Release, deletes the `<DEVKIT_TAG_PREFIX>X.Y.Z` tag, closes the release PR with an audit comment, and deletes `release/X.Y.Z`. The version number remains available for a re-cut; RC artifacts are **not** pruned (a re-cut of the same version reclaims them at its promote). Every step is idempotent, so a partially failed run can simply be re-dispatched.

This is the explicit, guarded exception to the no-tag-deletion rollback policy above: it is safe **only while the Release is a draft**, and the workflow hard-refuses a published release ([#1511](https://github.com/vig-os/devkit/issues/1511)). Publishing tombstones the tag name permanently — after promote, the only path is fixing forward with the next version.

## Hotfix lane (gitflow only)

An urgent fix (typically security) that must ship as a patch **without carrying whatever `dev` has accumulated** goes through **`prepare-hotfix.yml`** (`just prepare-hotfix X.Y.Z`; dispatches on `dev` by default — the ref only picks which copy of the workflow file runs, the workflow itself checks out `main`) instead of `prepare-release.yml` ([#1621](https://github.com/vig-os/devkit/issues/1621), scaffold port [#1625](https://github.com/vig-os/devkit/issues/1625)). Everything after prepare — candidate, final, promote, abandon, the sync back to `dev` — is the regular train, unchanged.

**Preconditions (enforced by `validate`; `dry-run: true` runs only that job):**

- `version` is exactly `MAJOR.MINOR.(PATCH+1)` of the highest stable `<DEVKIT_TAG_PREFIX>X.Y.Z` tag reachable from `main` — no hotfix minors, no patches of an older line, no skipped numbers.
- **No other `release/*` branch exists** (single-train policy; `prepare-release.yml` refuses symmetrically while a hotfix branch is in flight, [#1627](https://github.com/vig-os/devkit/issues/1627)). Promote or abandon the other train first.
- No `release/X.Y.Z` branch and no `<DEVKIT_TAG_PREFIX>X.Y.Z` tag exist.
- `main`'s `## Unreleased` is **classified, not required to be empty**. It may carry changes that landed on `main` but have not shipped; a hotfix cuts from `main`'s head and therefore *ships* them, so `validate` records which shape it found and the branch is written accordingly. The only hard refusal left here is a missing `## Unreleased` or a pre-existing `[X.Y.Z]` section.

**What the workflow does:** forks `release/X.Y.Z` at `main`'s head (re-checked against the validated SHA), writes the `## [X.Y.Z] - TBD` section **on the release branch** (never `main`, never `dev`) — freezing `main`'s carried entries into it when there are any, otherwise seeding it **empty** — runs the [`prepare-release-extension.yml` hook](#prepare-release-extension-hook) with that commit as `branch_sha`, and opens the draft PR `release/X.Y.Z` → `main`. `main`'s `## Unreleased` self-clears when the release branch merges back. Rollback on failure or cancellation deletes the partial branch and nothing else. Every job runs on the mode-aware devkit toolchain; no checkout keeps git credentials.

**Then, in order:**

1. **Land the fix** via a `bugfix/<issue>-<summary>` PR into `release/X.Y.Z`. The PR **must describe the fix in `## [X.Y.Z] - TBD`**: `release-core.yml` refuses to publish a version whose section is still empty (`prepare-changelog validate --version`), candidates and finals alike. That gate checks that the section is non-empty and that its bullets sit under a recognised `###` heading ([#1689](https://github.com/vig-os/devkit/issues/1689)) — so write the fix under `### Fixed`, not loose under the version heading. When the section was frozen from `main`'s carried entries it is already non-empty, so describing the fix itself stays your job.
2. Run the regular train unchanged: `just publish-candidate X.Y.Z`, `gh pr ready`, `just finalize-release X.Y.Z`, approve, `just promote-release X.Y.Z` ([Promote release (final)](#promote-release-final)).
3. **Resolve the sync-back conflict.** The post-promote `sync-main-to-dev` PR **will conflict on `CHANGELOG.md`** whenever `dev` is ahead: the regular cycle's conflict-free merge is bought by the shared freeze commit, which cannot exist for content authored off `main`. The sync workflow's manual-conflict lane hands it over (`merge-conflict` label, instructions in the PR body):

   ```bash
   git fetch origin chore/sync-main-to-dev-<id>:chore/sync-main-to-dev-<id>
   git checkout chore/sync-main-to-dev-<id>
   git merge origin/dev
   # CHANGELOG.md: keep dev's "## Unreleased" and its bullets at the top,
   # insert main's dated "## [X.Y.Z] - YYYY-MM-DD" section (link included) directly
   # beneath it (above the previous release), keep everything else from dev.
   git add CHANGELOG.md
   git commit
   git push origin chore/sync-main-to-dev-<id>
   ```

**Runbook rules:**

- **One train at a time, in both directions.** `prepare-hotfix` refuses while any other `release/*` branch exists and `prepare-release` refuses while a hotfix branch is in flight ([#1627](https://github.com/vig-os/devkit/issues/1627)). With floating tags enabled, `promote-release` additionally refuses a version below the highest published final release, so a hand-made second train cannot move `<prefix>X` / `<prefix>X.Y` backwards ([#1626](https://github.com/vig-os/devkit/issues/1626)): abandon the stale train and re-cut the fix as the next patch of the published line.
- **Expect the promote BEHIND gate** if anything lands on `main` mid-hotfix; recovery is merging `main` into the release branch (which dismisses the approval — approve again afterwards).
- **`release.yml` runs from the release branch's copy — `main`'s copy for a hotfix.** A release-workflow change the lane depends on (a devkit adoption PR merged to `dev`) reaches hotfix trains only once it has shipped to `main` through a regular train.
- **Rehearsing the lane** (no train in flight): `prepare-hotfix`, a trivial fix PR, one `publish-candidate`, then `abandon-release`. Never finalize or promote a rehearsal. The `X.Y.Z-rcN` git tag stays (numbering continuity), and a candidate created with `create-release=true` leaves a pre-release that is immutable once published.

## Workflow models

The whole release flow above is the same under either **workflow model** a
consumer selects with `DEVKIT_WORKFLOW` in `.vig-os` (`gitflow`, the default, or
`trunk` — [#1205](https://github.com/vig-os/devkit/issues/1205)): `release/X.Y.Z`
is still cut, driven through the RC train, finalized, and merged. The models
differ only in the base the release branch forks from and merges back to, which
is settled entirely at scaffold time (an anchored `dev -> main` render — see
[`docs/rfcs/ADR-workflow-model.md`](https://github.com/vig-os/devkit/blob/main/docs/rfcs/ADR-workflow-model.md)).
Two release steps are model-dependent:

- **The changelog freeze targets `dev` under `gitflow` and `release/X.Y.Z` under
  `trunk`** ([#1479](https://github.com/vig-os/devkit/issues/1479)).
  `prepare-release.yml` cuts `release/X.Y.Z` from the base *before* it freezes,
  then commits the freeze to the model's freeze target and fast-forwards the
  release branch onto it (a no-op under `trunk`, where the freeze already landed
  there). Under `trunk` the base branch is also the release PR's base, so
  freezing onto it would leave head and base at the same commit — GitHub then
  refuses to open the PR — and would push straight to the trunk. Consequence for
  a trunk consumer: **the Commit App needs no bypass on the `main` ruleset**; it
  only ever writes to `release/*`. `main` receives the frozen changelog when the
  release PR merges at promote time.
- **`sync-main-to-dev.yml` runs only under `gitflow`.** The gitflow model keeps a
  long-lived `dev` integration branch, so a push to `main` (including a release
  merge) opens a PR syncing `main` back into `dev`. The `trunk` model has no
  `dev` branch — `feature`/`bugfix`/`chore` branches merge straight to `main` and
  `release/X.Y.Z` merges back into `main` — so `sync-main-to-dev.yml` is never
  scaffolded (copy-excluded, and pruned on a gitflow → trunk upgrade). The
  promote-time back-merge referenced above is therefore a no-op under `trunk`.
- **`prepare-hotfix.yml` runs only under `gitflow`.** The [hotfix lane](#hotfix-lane-gitflow-only)
  exists to cut a patch from `main` *instead of* `dev`; under `trunk` every
  release already cuts from `main`, so the lane is redundant there and is
  never scaffolded — copy-excluded and pruned on a gitflow → trunk upgrade
  exactly like `sync-main-to-dev.yml`, with the `just prepare-hotfix` recipe
  dropped from the scaffolded `justfile.gh` ([#1625](https://github.com/vig-os/devkit/issues/1625)).

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

`promote-release.yml` is a standalone `workflow_dispatch` workflow (input: `version`); it does not call the reusable workflows above. `prepare-hotfix.yml` (gitflow only) is likewise standalone (inputs: `version`, `dry-run`) and calls only the [prepare-release extension hook](#prepare-release-extension-hook).

## Toolchain provisioning is mode-aware

Since [#991](https://github.com/vig-os/devkit/issues/991), the whole
release/automation set provisions its toolchain per `DEVKIT_MODE`
(`.vig-os`), following the conditional-`container:` pattern in
[`docs/rfcs/ADR-conditional-container-toolchain.md`](https://github.com/vig-os/devkit/blob/main/docs/rfcs/ADR-conditional-container-toolchain.md):

- Each `workflow_dispatch`/event-triggered workflow (`release.yml`,
  `prepare-release.yml`, `promote-release.yml`, `sync-issues.yml`,
  `sync-main-to-dev.yml`) runs a leading
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
(`prepare-changelog`, `synthesize-bot-changelog`) come from the composite, so the
choreography's bare `run:` invocations are identical in every mode. In `bare`
mode the composite pins `vig-utils` to the `.vig-os` `DEVKIT_VERSION`
(`synthesize-bot-changelog` and `prepare-changelog` in `prepare-release.yml` /
`release-core.yml`); see
[`docs/MIGRATION.md`](https://github.com/vig-os/devkit/blob/main/docs/MIGRATION.md#bare-mode-vig-utils-release-console-scripts).

## Required App Secrets

Downstream repositories are expected to provide both app credentials:

- `COMMIT_APP_CLIENT_ID`
- `COMMIT_APP_PRIVATE_KEY`
- `RELEASE_APP_CLIENT_ID`
- `RELEASE_APP_PRIVATE_KEY`

Template behavior relies on explicit app-token generation for release operations:

- use **Commit App** token for protected branch/ref writes (`commit-action`, branch/tag mutation)
- use **Release App** token for release orchestration and PR/release API operations

`github.token` is intentionally not used as a fallback for these release write paths.

## Input Naming Convention

All `workflow_call` inputs use underscores (e.g. `release_kind`, `dry_run`, `tag_prefix`). The orchestrator `release.yml` translates its own `workflow_dispatch` hyphenated inputs at each call site.

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
- `branch_sha` — the post-freeze head SHA of the release branch (the changelog-freeze commit)
- `dry_run` — validate without making changes (extensions must honor it)

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

Two independent staleness axes are reported in CI ([#1497](https://github.com/vig-os/devkit/issues/1497)):

- **Scaffold drift** (`scaffold-drift` job, gate): the working tree diverges from what the *pinned* `DEVKIT_VERSION` would scaffold. Opt out with `DEVKIT_DRIFT_CHECK=false`.
- **Pin staleness** (`devkit-staleness` job, warn-only): the pin itself is behind the latest devkit release — invisible to the drift gate by construction, since it compares the pin against itself. The report is a `::warning` annotation plus a step-summary block; it never fails the build and is not silenced by the drift opt-out.

The flake-input axis is reported by the upgrade lane itself: `install.sh --force` prints one `flake-bump:` line per run — advanced (any input name at the floating `github:vig-os/devkit` URL), or skipped with the reason (a pinned ref, in either `?ref=X` or `/X` form, is never auto-bumped) — and `devkit-upgrade.yml` carries that line into the adoption PR body.
