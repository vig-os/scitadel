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

# Pre-trust workspace for agent/conductor (add to ~/.cursor/cli-config.json trustedDirectories)
if command -v agent >/dev/null 2>&1; then
    echo "Adding workspace to Cursor agent trustedDirectories..."
    cfg="${HOME}/.cursor/cli-config.json"
    mkdir -p "$(dirname "$cfg")"
    [ ! -f "$cfg" ] && echo '{}' > "$cfg"
    if ! jq -e --arg d "$PROJECT_ROOT" '.trustedDirectories // [] | index($d)' "$cfg" >/dev/null 2>&1; then
        jq --arg d "$PROJECT_ROOT" '.trustedDirectories = ((.trustedDirectories // []) + [$d])' "$cfg" > "${cfg}.tmp" \
            && mv "${cfg}.tmp" "$cfg"
        echo "[OK] Trusted directory added: $PROJECT_ROOT"
    else
        echo "[OK] Directory already trusted: $PROJECT_ROOT"
    fi
fi

# User specific setup
# Add your custom setup commands here to install any dependencies or tools needed for your project

echo "Post-create setup complete"
