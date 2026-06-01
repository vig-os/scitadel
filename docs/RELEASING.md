# Releasing

Releases are automated with [release-please](https://github.com/googleapis/release-please).
You write [conventional commits](https://www.conventionalcommits.org/); the
pipeline does the rest.

## Flow

1. Land conventional commits on `main` (`feat:`, `fix:`, `feat!:`/`BREAKING CHANGE:`).
2. `release-please.yml` keeps an open **release PR** that bumps the version and
   updates `CHANGELOG.md`. Review it as commits accumulate.
3. **Merge the release PR.** release-please then:
   - bumps all 8 crate `[package].version`s and their inter-crate dependency
     requirements in lockstep (`cargo-workspace` plugin) and updates `Cargo.lock`,
   - creates one unprefixed git tag (e.g. `0.7.0`) and a GitHub Release.
4. The tag triggers:
   - `publish-crates.yml` → publishes all crates to crates.io in dependency order,
   - `binaries.yml` → builds per-platform tarballs and attaches them to the Release.

## Versioning model

- Single shared version across the workspace. Each `crates/scitadel-*/Cargo.toml`
  carries an explicit `version = "x.y.z"` (NOT `version.workspace = true` — the
  `cargo-workspace` plugin requires a literal version to bump). There is no
  `[workspace.package].version`; release-please owns the per-crate versions.
- release-please config: [`release-please-config.json`](../release-please-config.json),
  state: [`.release-please-manifest.json`](../.release-please-manifest.json).
- Tags are unprefixed `X.Y.Z` (`include-v-in-tag: false`) to match the
  tag triggers in `publish-crates.yml` and `binaries.yml`.

## One-time setup (repo secrets)

| Secret | Why |
| --- | --- |
| `RELEASE_PLEASE_TOKEN` | Fine-grained PAT (`contents: write`, `pull-requests: write`). Required so the tag release-please pushes **triggers** `publish-crates.yml`/`binaries.yml` — tags pushed with the default `GITHUB_TOKEN` do not trigger other workflows. Without it the release PR + Release are still created, but publish/binaries won't auto-run. |
| `CARGO_REGISTRY_TOKEN` | crates.io API token used by `publish-crates.yml` at tag time. |

## First-run checklist

release-please's first release PR for a workspace is worth eyeballing before merge:

- [ ] exactly **one** tag will be cut (no per-crate tag collisions),
- [ ] all 8 `crates/scitadel-*/Cargo.toml` `version`s are bumped,
- [ ] all internal `scitadel-* = { path = …, version = "…" }` requirements are bumped,
- [ ] `Cargo.lock` updated,
- [ ] `CHANGELOG.md` reads correctly.

## Plugin version pin

The Claude Code plugin fetches a pinned release binary. After a release, bump
`plugins/scitadel/bin/VERSION` and `plugins/scitadel/.claude-plugin/plugin.json`
`version` to the new tag so the plugin installs the matching binary.

## Tag hygiene note

Earlier tags are inconsistent (`v0.5.0` prefixed vs `0.3.0` unprefixed); only the
unprefixed form triggers the publish/binaries workflows. Going forward
release-please emits unprefixed tags exclusively. The stray `v*` tags are
harmless but can be deleted for tidiness if desired.
