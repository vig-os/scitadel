#!/usr/bin/env bash
# Check that all skill directory names under a given path use only
# lowercase letters, digits, hyphens, and underscores.
#
# Usage: check-skill-names.sh [skills_dir]
#   skills_dir  Path to scan (default: .cursor/skills)
#
# Exit 0 if all names are valid, 1 if any are invalid.

set -euo pipefail

skills_dir="${1:-.cursor/skills}"

if [[ ! -d "$skills_dir" ]]; then
    echo "Error: directory not found: $skills_dir" >&2
    exit 1
fi

invalid=()

for dir in "$skills_dir"/*/; do
    [[ -d "$dir" ]] || continue
    name="$(basename "$dir")"
    if [[ ! "$name" =~ ^[a-z0-9][a-z0-9_-]*$ ]]; then
        invalid+=("$name")
    fi
done

if [[ ${#invalid[@]} -gt 0 ]]; then
    echo "Invalid skill directory name(s) — must match [a-z0-9][a-z0-9_-]*:" >&2
    for name in "${invalid[@]}"; do
        echo "  $name" >&2
    done
    exit 1
fi
