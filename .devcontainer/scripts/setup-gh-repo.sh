#!/bin/bash
set -euo pipefail

# Configure GitHub repository settings for merge commit compliance.
# Sets merge commit format to use PR title/body and enables auto-merge.
# Called from post-create.sh (postCreateCommand) — runs once per container.
#
# Requires: gh CLI authenticated with repo admin permissions.
# Fails gracefully if gh is not authenticated or lacks permissions.

REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>/dev/null || echo "")

if [ -z "$REPO" ]; then
	echo "Skipping repo merge settings (not in a GitHub repo or gh not authenticated)"
	exit 0
fi

echo "Configuring merge commit settings for $REPO..."

if gh api "repos/$REPO" -X PATCH \
	-f merge_commit_title=PR_TITLE \
	-f merge_commit_message=PR_BODY \
	-F allow_auto_merge=true \
	>/dev/null 2>&1; then
	echo "✓ Merge commit format: PR title + body"
	echo "✓ Auto-merge: enabled"
else
	echo "✗ Could not update repo settings (insufficient permissions?)"
	echo "  Manual setup: gh api repos/$REPO -X PATCH -f merge_commit_title=PR_TITLE -f merge_commit_message=PR_BODY -F allow_auto_merge=true"
fi
