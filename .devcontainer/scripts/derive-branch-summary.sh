#!/usr/bin/env bash
# Derive a kebab-case branch summary from an issue title.
# Used by worktree-start when no linked branch exists.
#
# Usage: derive-branch-summary.sh <TITLE> [NAMING_RULE] [MODEL_TIER]
#   TITLE: issue title
#   NAMING_RULE: path to branch-naming.mdc (default: .cursor/rules/branch-naming.mdc)
#   MODEL_TIER: agent-models.toml tier (default: lightweight). Use standard for retry.
#
# Env: BRANCH_SUMMARY_CMD — override for tests (e.g. "echo test-summary")
#      When set, runs instead of agent. Must output summary to stdout.
#      BRANCH_SUMMARY_MODEL — override model tier (e.g. "standard"). Ignored if BRANCH_SUMMARY_CMD set.
#      DERIVE_BRANCH_TIMEOUT — timeout in seconds (default: 30). Use 2 for tests.
set -euo pipefail

TITLE="${1:?Usage: derive-branch-summary.sh <TITLE> [NAMING_RULE] [MODEL_TIER]}"
REPO_ROOT="$(git rev-parse --show-toplevel)"
NAMING_RULE="${2:-${REPO_ROOT}/.cursor/rules/branch-naming.mdc}"
MODEL_TIER="${3:-${BRANCH_SUMMARY_MODEL:-lightweight}}"
TIMEOUT="${DERIVE_BRANCH_TIMEOUT:-30}"

if [ -n "${BRANCH_SUMMARY_CMD:-}" ]; then
    SUMMARY=$(timeout "$TIMEOUT" sh -c "$BRANCH_SUMMARY_CMD" 2>/dev/null | tail -1 | tr -d '[:space:]') || true
else
    MODEL=$(grep "^${MODEL_TIER}" "${REPO_ROOT}/.cursor/agent-models.toml" | sed 's/.*= *"//' | sed 's/".*//')
    SUMMARY=$(timeout "$TIMEOUT" agent --print --yolo --trust --model "$MODEL" \
        "Read the branch naming rules in ${NAMING_RULE}. " \
        "The issue title is: ${TITLE} " \
        "Output ONLY a kebab-case short summary suitable for a branch name (a few words). " \
        "Omit prefixes like FEATURE, BUG, Add, Implement, Support. " \
        "Example: 'Standardize and Enforce Commit Message Format' -> 'standardize-commit-messages'. " \
        "No explanation. No quotes. Just the summary." 2>/dev/null | tail -1 | tr -d '[:space:]') || true
fi

if [ -z "$SUMMARY" ]; then
    echo "[ERROR] Failed to derive branch summary from title: ${TITLE}" >&2
    echo "        Create one manually: gh issue develop <ISSUE> --base dev --name <type>/<issue>-<summary>" >&2
    exit 1
fi

echo "$SUMMARY"
