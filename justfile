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
