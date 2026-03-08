#!/bin/bash

# Post-create script - runs once when container is created for the first time.
# This script is called from postCreateCommand in devcontainer.json.
#
# All one-time setup belongs here:
#   - Git repo init, config, hooks
#   - SSH key + allowed-signers placement
#   - GitHub CLI config + authentication
#   - Pre-commit hook installation
#   - Dependency sync (via just)

set -euo pipefail

echo "Running post-create setup..."

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="/workspace/scitadel"

if [ ! -d "$PROJECT_ROOT" ]; then
    echo "Error: Project directory $PROJECT_ROOT does not exist"
    exit 1
fi

# Set venv prompt
sed -i 's/template-project/scitadel/g' /root/assets/workspace/.venv/bin/activate

# One-time setup: git repo, config, hooks, gh auth
"$SCRIPT_DIR/init-git.sh"
"$SCRIPT_DIR/setup-git-conf.sh"
"$SCRIPT_DIR/setup-gh-repo.sh"
"$SCRIPT_DIR/init-precommit.sh"

# Sync dependencies (fast if nothing changed from pre-built venv)
echo "Syncing dependencies..."
just --justfile "$PROJECT_ROOT/justfile" --working-directory "$PROJECT_ROOT" sync

# Claude Code setup
# Create /.dockerenv so Claude Code can detect it's running in a container.
# Podman doesn't create this file (Docker does), which causes Claude Code to
# refuse --dangerously-skip-permissions when running as root (standard Podman issue).
touch /.dockerenv

# Install Claude Code if not already present
if ! command -v claude >/dev/null 2>&1; then
    echo "Installing Claude Code..."
    npm install -g @anthropic-ai/claude-code
fi

echo "Post-create setup complete"
