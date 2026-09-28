# Managed by vigOS devkit — regenerated on upgrade; local edits are lost.
# Customize in justfile.project. Bugs / missing tools: https://github.com/vig-os/devkit/issues

# ===============================================================================
# MAIN JUSTFILE - Orchestrates all recipe sources
# ===============================================================================

# Run every recipe under a strict bash so pipelines fail on the first error.
# Lives here (not justfile.devc) so it applies in ALL delivery modes — direnv
# and bare have no .devcontainer/justfile.devc, yet their justfile.project
# recipes must still get pipefail (#854).
set shell := ["bash", "-euo", "pipefail", "-c"]

# Show available commands
[group('info')]
help:
    @just --list

# Diagnostics only — always exits 0 (#1448). Lives here, not in
# justfile.project: this is the only managed justfile layer present in EVERY
# delivery mode (direnv/bare carry no .devcontainer/), and justfile.project is
# preserved on upgrade, so a recipe placed there would never reach an existing
# consumer.

# Diagnose host prerequisites: git identity, signing, hooks path, ssh-agent, gh auth
[group('info')]
doctor:
    #!/usr/bin/env bash
    echo "vigOS devkit doctor"
    echo "==================="

    name="$(git config user.name || true)"
    if [ -n "$name" ]; then
        echo "PASS git user.name: $name"
    else
        echo "WARN git user.name: not set (git config --global user.name ...)"
    fi

    email="$(git config user.email || true)"
    if [ -n "$email" ]; then
        echo "PASS git user.email: $email"
    else
        echo "WARN git user.email: not set (git config --global user.email ...)"
    fi

    gpgsign="$(git config commit.gpgsign || true)"
    format="$(git config gpg.format || true)"
    signingkey="$(git config user.signingkey || true)"
    # git expands a leading `~/` itself when it consumes user.signingkey, but
    # `test -r` does not — the shell expands `~` only as an unquoted literal at
    # the start of a word, never the contents of a variable. Expand it here too,
    # or a working tilde-path setup reads as incomplete (#1546). The PASS line
    # keeps reporting the raw value, so it still mirrors git config.
    keypath="$signingkey"
    case "$keypath" in
        "~/"*) keypath="$HOME/${keypath#\~/}" ;;
    esac
    if [ "$gpgsign" = "true" ] && [ -n "$signingkey" ] && \
        { [ "$format" != "ssh" ] || [ -r "$keypath" ] || \
          [ "${signingkey#ssh-}" != "$signingkey" ]; }; then
        echo "PASS commit signing: $format key $signingkey"
    else
        echo "WARN commit signing: incomplete (commit.gpgsign=$gpgsign, gpg.format=$format, user.signingkey=$signingkey)"
    fi

    # .githooks is tracked, so a fresh clone has the shims on disk — but git
    # ignores them until core.hooksPath points there. It is LOCAL repo config,
    # so it is never cloned: until something sets it, every commit-side gate is
    # present, believed active, and inert (#1430). What sets it depends on the
    # delivery mode recorded in .vig-os: the devcontainer setup
    # (setup-git-conf.sh) or dev-shell entry (#1112). `bare` has neither, and
    # neither does an ad-hoc checkout — so the direct git command is always the
    # primary fix and the mode entry point is named on top of it. Refs #1448.
    mode="$(sed -n 's/^DEVKIT_MODE=//p' "{{ justfile_directory() }}/.vig-os" 2>/dev/null | head -n1 | tr -d '[:space:]')"
    hint="(run: git config core.hooksPath .githooks)"
    case "$mode" in
        devcontainer) hint="$hint; normally set by the devcontainer setup: reopen the container" ;;
        direnv)       hint="$hint; normally set on dev-shell entry: run direnv allow" ;;
        both)         hint="$hint; normally set by the devcontainer setup (reopen the container) or on dev-shell entry (direnv allow)" ;;
    esac

    # A linked worktree may legitimately run with core.hooksPath unset and live
    # shims in the shared git dir (#1454). Since #1463 `just worktree-start`
    # leaves a configured hooks path untouched — the relative path resolves
    # against each worktree's root and .githooks is tracked, so a post-fix
    # worktree keeps the shared setting and hits the normal .githooks PASS
    # branch — and prek-installs into the shared .git/hooks only as the
    # fallback when no hooks path is configured at all. The shared-hooks PASS
    # branch below covers that fallback plus worktrees created before #1463
    # (worktree-start used to unset core.hooksPath — shared config, the #1463
    # bug — and always install): `hooks` is one of git's shared paths, so the
    # shims land in the common git dir and git runs them from inside the
    # worktree — the gates are live, and the remediation above would undo the
    # setup. Claim that only when a shim is really installed and executable: a
    # worktree without one is inert exactly like any other unset case.
    # `--git-path hooks/pre-commit` resolves to the file git itself would run
    # (`.git` is a FILE in a linked worktree, so a literal `.git/hooks/...`
    # test could never see it).
    hookspath="$(git config core.hooksPath || true)"
    gitdir="$(git rev-parse --absolute-git-dir 2>/dev/null || true)"
    commondir="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
    installed="$(git rev-parse --git-path hooks/pre-commit 2>/dev/null || true)"
    if [ "$hookspath" = ".githooks" ]; then
        echo "PASS git hooks: core.hooksPath -> .githooks"
    elif [ -z "$hookspath" ] && [ -n "$gitdir" ] && [ "$gitdir" != "$commondir" ] && [ -x "$installed" ]; then
        echo "PASS git hooks: linked worktree, installed at $installed (core.hooksPath unset by design)"
    elif [ -z "$hookspath" ]; then
        echo "WARN git hooks: core.hooksPath not set, .githooks is tracked but inert $hint"
    else
        echo "WARN git hooks: core.hooksPath=$hookspath, expected .githooks $hint"
    fi

    if [ -n "${SSH_AUTH_SOCK:-}" ] && ssh-add -l >/dev/null 2>&1; then
        echo "PASS ssh-agent: reachable with $(ssh-add -l | wc -l) key(s)"
    else
        echo "WARN ssh-agent: not reachable or no keys loaded"
    fi

    if gh auth status >/dev/null 2>&1; then
        echo "PASS gh auth: logged in"
    else
        echo "WARN gh auth: not authenticated (run: gh auth login)"
    fi

    exit 0

# Run a command with libstdc++ resolvable for uvx-run native wheels (#1181).
# On a non-Python direnv-mode consumer the CI preamble keeps the Nix CPython on
# PATH, whose loader does not search /usr/lib, so a uvx tool's manylinux native
# wheel (e.g. otterdog's rjsonnet) fails to import with
# "libstdc++.so.6: cannot open shared object file". This wraps ONE command with
# a command-scoped LD_LIBRARY_PATH sourced from $VIGOS_STDCPP_LIB (dev-shell
# export) or derived from the on-PATH `cc` wrapper (cc echoes the bare name
# back — not an absolute path — when it cannot resolve the lib, so the leading-/
# check leaves the prefix empty). Scoping it to this one command keeps the Nix
# libstdc++ out of every other tool's process. When nothing resolves, the
# environment is left UNTOUCHED — never compose with an empty prefix, since an
# empty LD_LIBRARY_PATH entry (leading colon, or a bare "") means "current
# working directory" to the dynamic loader. Lives here (not justfile.devc,
# devcontainer-only) so it is reachable in direnv/bare mode — the case that
# needs it. Usage from a justfile.project recipe:
# just with-native-libs uvx --from otterdog@1.2.3 otterdog validate --local
[group('helpers')]
with-native-libs +command:
    @stdcpp_lib="${VIGOS_STDCPP_LIB:-}"; \
    if [ -z "$stdcpp_lib" ] && command -v cc >/dev/null 2>&1; then \
      p="$(cc -print-file-name=libstdc++.so.6)"; \
      case "$p" in /*) stdcpp_lib="$(dirname "$p")" ;; esac; \
    fi; \
    if [ -n "$stdcpp_lib" ]; then \
      LD_LIBRARY_PATH="$stdcpp_lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" {{ command }}; \
    else \
      {{ command }}; \
    fi

# Import devcontainer-managed base recipes (replaced on upgrade).
# Optional: a `direnv`-mode workspace (`init-workspace --mode direnv`) has no
# .devcontainer/, so these must not be hard imports or `just` fails to load.

import? '.devcontainer/justfile.devc'
import? '.devcontainer/justfile.gh'
import? '.devcontainer/justfile.worktree'

# Import team-shared project recipes (git-tracked, preserved on upgrade)

import? 'justfile.project'

# Import personal recipes (gitignored, preserved on upgrade)

import? 'justfile.local'
