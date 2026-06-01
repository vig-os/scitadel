#!/usr/bin/env bash
# Store a Personal Access Token as the RELEASE_PLEASE_TOKEN repo secret.
#
# GitHub has NO API to *mint* a PAT — that step is web-UI only (see
# docs/release-token-setup.md). This script automates the half that CAN be
# scripted: securely reading the token and setting it as the repo secret.
#
# Usage:
#   scripts/setup-release-token.sh                 # prompts for the token (hidden)
#   echo "github_pat_xxx" | scripts/setup-release-token.sh   # piped (CI-friendly)
set -euo pipefail

REPO="${REPO:-vig-os/scitadel}"
SECRET="${SECRET:-RELEASE_PLEASE_TOKEN}"

command -v gh >/dev/null 2>&1 || { echo "error: gh CLI not found (https://cli.github.com)"; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "error: not logged in — run: gh auth login"; exit 1; }

if [ -t 0 ]; then
  printf 'Paste the PAT for %s on %s (input hidden), then Enter:\n' "$SECRET" "$REPO"
  read -rs TOKEN
  echo
else
  read -r TOKEN   # piped value
fi
[ -n "${TOKEN:-}" ] || { echo "error: empty token"; exit 1; }

# Pipe via stdin so the token never appears in argv / process list.
printf '%s' "$TOKEN" | gh secret set "$SECRET" --repo "$REPO"
unset TOKEN

if gh secret list --repo "$REPO" | grep -qE "^$SECRET\b"; then
  echo "✅ $SECRET is set on $REPO"
else
  echo "error: $SECRET not found after set"; exit 1
fi
