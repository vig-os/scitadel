---
type: issue
state: closed
created: 2026-08-07T13:06:14Z
updated: 2026-09-28T12:38:19Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/209
comments: 1
labels: chore, effort:medium, priority:medium, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:26.956Z
---

# [Issue 209]: [chore(ci): bump devkit to 1.6.0 and migrate release-bot to client-id](https://github.com/vig-os/scitadel/issues/209)

## Context

A 2026-08-07 fleet audit of GitHub-App credentials across `vig-os` and `exo-pet`
concluded that every App secret pair can be consolidated to **Client ID only**.
Nothing in the auth path is numerically load-bearing: `@octokit/auth-app`
accepts a client-ID string (`Iv23li…`) wherever it accepts a numeric App ID, on
every pinned action version in use. `actions/create-github-app-token` grew a
`client-id` input in **v3.1.0**.

`scitadel` has **two** independent numeric-ID surfaces, and they need different
treatment.

## Part (a) — the devkit scaffold (tracked in #207)

`main` runs a **0.3.3-era** scaffold whose App tokens are minted with
`actions/create-github-app-token@1b10c78c7865c340bc4f6099eb2f838309f1e8c3`
(v3.1.1 — supports `client-id`, but the scaffold still writes `app-id`) and
whose sync step pins `vig-os/sync-issues-action@bad447d33…` (v0.2.2, no
`client-id` input at all).

- `.github/workflows/sync-issues.yml:81,83` — `create-github-app-token` +
  `app-id: ${{ secrets.COMMIT_APP_ID }}`
- `.github/workflows/sync-issues.yml:124,126` — `sync-issues-action` +
  `app-id: ${{ secrets.COMMIT_APP_ID }}`

Same rationale as vig-os/h5v#6: the devkit **1.6.0** bump converts these to the
client-ID scaffolds wholesale. **That bump is already tracked in #207** (branch
`chore/207-upgrade-devkit-1-6-0` is in flight) — this issue does not duplicate
it; it records the credential requirement the bump must satisfy, and owns part
(b) below, which #207 does not cover.

The residual `sync-issues.yml` holdout in the devkit scaffold itself is fixed in
vig-os/devkit#1365 (blocked on vig-os/sync-issues-action#168). If #207 lands
first, `COMMIT_APP_ID` survives in `sync-issues.yml` until the next devkit bump —
expected, not a blocker.

## Part (b) — the repo-local release-please bot (owned here)

This one is **not** devkit-stamped and will not be fixed by any scaffold bump.

- **`.github/workflows/release-please.yml:46`** — `uses: actions/create-github-app-token@v1`,
  a **floating major tag** (unpinned; also a supply-chain / zizmor concern
  independent of this migration).
- **`:48`** — presence gate: `if: ${{ vars.RELEASE_BOT_APP_ID != '' }}`
- **`:50`** — `app-id: ${{ vars.RELEASE_BOT_APP_ID }}` (a repo **variable**, not
  a secret; `:51` takes `secrets.RELEASE_BOT_PRIVATE_KEY`)
- **`:25`** — header comment documenting `RELEASE_BOT_APP_ID` as the preferred
  credential
- **`scripts/setup-release-bot.sh:82`** — `gh variable set RELEASE_BOT_APP_ID …`,
  fed by an interactive prompt for the numeric App ID; also referenced in the
  script's header comment `:8` and in the ordering comment/echo at `:80` and `:84`
- **`docs/RELEASING.md:74`** — names `RELEASE_BOT_APP_ID` in the teardown list

### Work

1. Rename the variable to **`RELEASE_BOT_APP_CLIENT_ID`** and update the gate at
   `:48`, the input at `:50`, and the comment at `:25`.
2. **Pin `actions/create-github-app-token` to a v3.x SHA** (at or above v3.1.0)
   and switch the input to `client-id:`.
3. Update `scripts/setup-release-bot.sh` to prompt for and set the client ID
   (`:8`, `:80`, `:82`, `:84`). Keep the existing ordering invariant — the
   private-key secret is set **before** the variable, because the workflow gates
   on the variable's presence.
4. Update `docs/RELEASING.md:74`.
5. Create the new repo variable in GitHub **before** merging, so the gate does
   not silently skip the App-token step and fall through to
   `secrets.RELEASE_PLEASE_TOKEN` / `GITHUB_TOKEN` (`:55`).

### Otterdog pairing (config-first)

`vig-os/org-config` declares this variable at
`otterdog/vig-os/vig-os.jsonnet:504` as `orgs.newRepoVariable('RELEASE_BOT_APP_ID')`
with a **plaintext value (`3931309`)** — it is a variable, not a secret, so it is
committed in the clear. The rename must be paired with a config edit in
`vig-os/org-config` **before** the workflow change lands, or the next otterdog
`apply`/drift run will fight it. That config edit is tracked in `vig-os/org-config`
under an existing tracker being re-scoped separately.

## Acceptance criteria

- [ ] Devkit scaffold on 1.6.0 (via #207); no `RELEASE_APP_ID`-style numeric
      reference left in devkit-stamped workflows.
- [ ] `release-please.yml` uses a **SHA-pinned** `create-github-app-token` >= v3.1.0
      with `client-id: ${{ vars.RELEASE_BOT_APP_CLIENT_ID }}`.
- [ ] The presence gate at `:48` tests the new variable name.
- [ ] `setup-release-bot.sh` and `docs/RELEASING.md` reference only the client ID.
- [ ] `vig-os.jsonnet:504` renamed in `vig-os/org-config`, applied, and drift-clean.
- [ ] A release-please run is observed producing a PR authored by the bot
      identity (proving the gate did not silently skip).
- [ ] Only then is the old `RELEASE_BOT_APP_ID` variable deleted.

## Migration principle

*No numeric `*_APP_ID` secret or variable is deleted while any pinned workflow
still references it.* Cautionary precedent — `exo-pet/playground-carlos`'s
sync-issues job broke **silently for a week** when `COMMIT_APP_ID` was
unavailable: it kept reporting success while syncing nothing. The
`if: vars.… != ''` gate here has exactly the same failure mode — a missing
variable degrades to `GITHUB_TOKEN` with no error.

Related: #207, vig-os/h5v#6, vig-os/devkit#1365, vig-os/sync-issues-action#168.

---

# [Comment #1]() by [c-vigo]()

_Posted on September 28, 2026 at 12:38 PM_

Closing: **part (a) is done, part (b) is moot.**

**Part (a) — the devkit scaffold.** Satisfied and exceeded. #226 took the scaffold
to devkit **1.17.0**, past the 1.6.0 this issue targeted. The residual holdout
this issue explicitly anticipated — *"`COMMIT_APP_ID` survives in
`sync-issues.yml` until the next devkit bump — expected, not a blocker"* — is
gone: `sync-issues.yml` now uses `client-id: ${{ secrets.COMMIT_APP_CLIENT_ID }}`
at both the token-mint (:97) and the `sync-issues-action` (:177) sites, so
vig-os/devkit#1365 and vig-os/sync-issues-action#168 are resolved upstream.

The only numeric reference left anywhere in `.github/workflows/` is
`devkit-upgrade.yml:72`'s `APP_ID_LEGACY`, devkit's own **deprecated fallback**
(`client-id: ${{ secrets.DEVKIT_UPGRADE_APP_CLIENT_ID || secrets.DEVKIT_UPGRADE_APP_ID }}`).
It resolves empty here and is upstream's to remove (vig-os/devkit#1366) — not a
scitadel surface.

Incidentally, this issue's own diagnosis is *why* #226 could not use
`devkit-upgrade.yml`: the legacy `DEVKIT_UPGRADE_APP_ID` org secret is not scoped
to this repo, and the 1.6.0 workflow read only that key, so the repo could not
upgrade itself to the version that fixed its own upgrade path. Done via
`install.sh --force`; a verification dispatch on the merged scaffold now
**succeeds**, so future bumps use the workflow.

**Part (b) — the repo-local release-please bot.** Moot rather than completed. Its
work items target files that no longer exist:

| Target | State |
|---|---|
| `.github/workflows/release-please.yml` | deleted in #207 / #208 |
| `scripts/setup-release-bot.sh` | deleted in #207 / #208 |
| AC: *"a release-please run is observed producing a PR authored by the bot identity"* | unachievable — release-please is retired |

So the rename-to-client-ID work is unnecessary, not outstanding.

**Residue handed off.** This issue's migration principle — *no numeric `*_APP_ID`
is deleted while any pinned workflow still references it* — is now **satisfied**,
because nothing references them. The three dead credentials
(`RELEASE_BOT_APP_ID` = `3931309`, `RELEASE_BOT_PRIVATE_KEY`,
`RELEASE_PLEASE_TOKEN`) are tracked in **vig-os/org-config#295**, which leads
because the otterdog declaration must go first or the next `apply` re-creates the
variable. `CARGO_REGISTRY_TOKEN` is deliberately kept — `publish-crates.yml:118-131`
still uses it as the fallback when trusted publishing (#224) is unavailable.

One correction for anyone following the trail: this issue cites
`otterdog/vig-os/vig-os.jsonnet:504` for the variable; it has since moved to
**831**, inside the `orgs.newRepo('scitadel')` block at 806.

