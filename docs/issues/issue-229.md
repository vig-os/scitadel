---
type: issue
state: closed
created: 2026-09-28T12:14:08Z
updated: 2026-09-29T01:09:56Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/229
comments: 1
labels: bug, effort:small, priority:medium, area:workspace
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:24.700Z
---

# [Issue 229]: [fix(build): just lint fails locally — nix rustc 1.95 vs an #[allow] for a clippy 1.98 lint](https://github.com/vig-os/scitadel/issues/229)

## Summary

`just lint` fails in the dev shell while CI's `Lint` job passes. Local
development cannot run the lint gate it is supposed to mirror.

```
error: unknown lint: `clippy::unused_async_trait_impl`
  --> crates/scitadel-mcp/src/server.rs:1126
error: could not compile `scitadel-mcp` (lib) due to 1 previous error
error: Recipe `lint` failed on line 32 with exit code 101
```

## Cause

`crates/scitadel-mcp/src/server.rs:1125-1126` carries:

```rust
// (clippy 1.98 `unused_async_trait_impl`); the signature is rmcp's, not ours.
#[allow(clippy::unused_async_trait_impl)]
```

The lint was introduced in **clippy 1.98**. The dev shell resolves
`pkgs.rust-bin.stable.latest` from this repo's own pinned `nixpkgs` +
`rust-overlay`, which currently give **rustc/clippy 1.95.0 (2026-04-14)** — older
than the lint. `just lint` runs `cargo clippy -D warnings`, which escalates the
`unknown_lints` warning to an error.

CI is unaffected: `rust-ci.yml` uses `dtolnay/rust-toolchain@stable`, i.e. the
real current stable, which knows the lint. So the gate passes on CI and fails for
every developer.

`cargo test` is unaffected — it does not pass `-D warnings`. `just test` runs the
full suite green (517 tests).

## Not caused by the devkit 1.17.0 upgrade (#225)

`nix flake update vigos` moved only the `vigos` subtree; the `nixpkgs`,
`rust-overlay` and `flake-utils` nodes are byte-identical before and after
(`4bd9165a9165`, `8087ff1f47ff`, `11707dc2f618`). The failure reproduces on
`dev` without those changes.

## Options

1. **Advance the Rust pin** — `nix flake update nixpkgs rust-overlay` so the dev
   shell gets ≥1.98. Closes the local/CI divergence at the root, but relocks
   every other package in the shell; needs a full `just verify` + tape run.
2. **Pin the toolchain explicitly** — add a `rust-toolchain.toml` and have both
   the flake and CI read it, so local and CI cannot diverge again. More work,
   removes the whole class of bug.
3. **Widen the allow** — `#[allow(unknown_lints, clippy::unused_async_trait_impl)]`.
   One line, works on both toolchains, but silences genuine unknown-lint
   warnings in that module.

Option 2 is the durable fix; option 3 unblocks developers immediately.

## Acceptance Criteria

- [ ] `just lint` and `just verify` pass in a fresh `direnv` session
- [ ] CI's `Lint` job still passes
- [ ] Local and CI clippy versions are pinned to the same source, or the
      divergence is documented

Refs: #225
---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 01:09 AM_

Fixed in #235 (option 2). `rust-toolchain.toml` now pins 1.98.1, and both the flake (`fromRustupToolchainFile`, rust-overlay bumped) and every repo-owned workflow read the toolchain from it, so local and CI clippy can't drift apart again. `just lint` passes in the dev shell. How to bump it is in `docs/RELEASING.md`.

