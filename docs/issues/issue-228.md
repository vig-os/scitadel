---
type: issue
state: closed
created: 2026-09-28T12:13:45Z
updated: 2026-09-29T01:11:49Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/228
comments: 1
labels: chore, effort:medium, priority:medium, area:workspace
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:25.088Z
---

# [Issue 228]: [chore(repo): adopt the devkit governance hooks and activate core.hooksPath](https://github.com/vig-os/scitadel/issues/228)

## Context

Surfaced while upgrading to devkit 1.17.0 (#225). The upgrade prints a template
diff for the preserved `.pre-commit-config.yaml`; five hooks devkit ships have
never been folded in, absent since the 0.3.3 → 1.6.0 upgrade (#207).

Hooks in the 1.17.0 template but not in this repo:

| Hook | What it does | Applicable here? |
|---|---|---|
| `validate-commit-msg` | Conventional-Commit + `Refs:` gate | **yes** |
| `check-agent-identity` | rejects AI author/committer identities | **yes** |
| `prepare-commit-msg-strip-trailers` | strips AI attribution trailers | **yes** |
| `nixfmt` | formats `*.nix` | **yes** — this repo owns `flake.nix` |
| `just-fmt` | formats justfiles | **yes** — was present at 0.3.3, lost in #207 |
| `ruff`, `ruff-format` | Python lint/format | no (Rust repo) |
| `check-expirations` | `.trivyignore` / `.vulnixignore` expiry | no (neither file) |

## Why this matters

1. **The commit/branch-type knobs have no local enforcement.** The 1.17.0
   scaffold renders `DEVKIT_COMMIT_TYPES`, `DEVKIT_BRANCH_TYPES` and
   `DEVKIT_REFS_OPTIONAL_TYPES` into a `validate-commit-msg` hook this repo does
   not have. The renders are inert locally; only CI's `validate-commit-range`
   catches violations, i.e. after the push.
2. **The no-AI-attribution rule is not hook-enforced here.** The workspace
   `CLAUDE.md` states it is "additionally enforced by pre-commit hooks". For this
   repo it is not — neither `check-agent-identity` nor
   `prepare-commit-msg-strip-trailers` is present.
3. **`core.hooksPath` is unset**, so `.githooks/` never runs. Even the hooks that
   *are* configured only fire via an explicit `prek run`. Devkit's flake
   `shellHook` normally sets this; worth checking why it has not taken effect
   here (possibly the #1463 `worktree-start` interaction).

## Notes

All five entries are `language: system` / `entry: uv run <script>` hooks
resolving from the dev shell. `vig-utils` is already present via the pinned
`vigos` overlay, and `uv run <console-script>` is confirmed working in this repo
despite it having no `pyproject.toml` (the `shellcheck-composite-actions` hook
added in #225 uses exactly that form and passes). `nixfmt` needs adding to the
dev-shell package list.

Adopting `validate-commit-msg` changes the local commit workflow, so it is a
deliberate decision rather than a silent rider on an upgrade — hence this issue
rather than folding it into #225.

## Acceptance Criteria

- [ ] The five applicable hooks are in `.pre-commit-config.yaml` and pass
      `prek run --all-files`
- [ ] `nixfmt` is in the dev shell; `flake.nix` is formatted
- [ ] `core.hooksPath` resolves to `.githooks` in a fresh `direnv` session, and a
      commit with an AI trailer is rejected locally
- [ ] `just doctor` reports the hooks active

Refs: #225
---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 01:11 AM_

Done in #236. Four of the five hooks are adopted: `validate-commit-msg`, `check-agent-identity`, `prepare-commit-msg-strip-trailers` and `nixfmt` (added to the dev shell; `flake.nix` is formatted). `just-fmt` stays retired: under devkit 1.17.0 it still reflows the managed justfile banner, which trips Scaffold Drift. Why `core.hooksPath` was unset: this repo uses `pkgs.mkShell`, not devkit's `mkProjectShell`, so it never inherited the hooksPath fragment. The flake's shellHook now sets it, with the same guards (main worktree only, idempotent).

