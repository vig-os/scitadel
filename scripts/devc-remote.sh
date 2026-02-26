#!/usr/bin/env bash
###############################################################################
# devc-remote.sh - Remote devcontainer orchestrator
#
# Starts a devcontainer on a remote host via SSH and opens Cursor/VS Code.
# Handles SSH connectivity, pre-flight checks, container state detection,
# and compose lifecycle. URI construction delegated to Python helper.
#
# USAGE:
#   ./scripts/devc-remote.sh <ssh-host>[:<remote-path>]
#   ./scripts/devc-remote.sh --help
#
# Examples:
#   ./scripts/devc-remote.sh myserver
#   ./scripts/devc-remote.sh user@host:/opt/projects/myrepo
#   ./scripts/devc-remote.sh myserver:/home/user/repo
#
# Part of #70. See issue #152 for design.
###############################################################################

set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# CONFIGURATION
# ═══════════════════════════════════════════════════════════════════════════════

# shellcheck disable=SC2034
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ═══════════════════════════════════════════════════════════════════════════════
# LOGGING (matches init.sh patterns)
# ═══════════════════════════════════════════════════════════════════════════════

log_info() {
    echo -e "${BLUE}ℹ${NC}  $1"
}

log_success() {
    echo -e "${GREEN}✓${NC}  $1"
}

log_warning() {
    echo -e "${YELLOW}⚠${NC}  $1"
}

log_error() {
    echo -e "${RED}✗${NC}  $1"
}

show_help() {
    sed -n '/^###############################################################################$/,/^###############################################################################$/p' "$0" | sed '1d;$d'
    exit 0
}

parse_args() {
    SSH_HOST=""
    REMOTE_PATH="~"

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --help|-h)
                show_help
                ;;
            -*)
                log_error "Unknown option: $1"
                echo "Use --help for usage information"
                exit 1
                ;;
            *)
                if [[ -n "$SSH_HOST" ]]; then
                    log_error "Unexpected argument: $1"
                    exit 1
                fi
                # Parse SSH-style format: user@host:path or host:path
                if [[ "$1" =~ ^([^:]+):(.+)$ ]]; then
                    SSH_HOST="${BASH_REMATCH[1]}"
                    REMOTE_PATH="${BASH_REMATCH[2]}"
                else
                    SSH_HOST="$1"
                    # Default to ~ (expanded by remote shell) if no path specified
                    REMOTE_PATH="~"
                fi
                shift
                ;;
        esac
    done

    if [[ -z "$SSH_HOST" ]]; then
        log_error "Missing required argument: <ssh-host>[:<remote-path>]"
        echo "Use --help for usage information"
        exit 1
    fi
}

detect_editor_cli() {
    if command -v cursor &>/dev/null; then
        # shellcheck disable=SC2034
        EDITOR_CLI="cursor"
    elif command -v code &>/dev/null; then
        # shellcheck disable=SC2034
        EDITOR_CLI="code"
    else
        log_error "Neither cursor nor code CLI found. Install Cursor or VS Code and enable the shell command."
        exit 1
    fi
}

check_ssh() {
    if ! ssh -o ConnectTimeout=5 -o BatchMode=yes "$SSH_HOST" true 2>/dev/null; then
        log_error "Cannot connect to $SSH_HOST. Check your SSH config and network."
        exit 1
    fi
}

remote_preflight() {
    local preflight_output
    # shellcheck disable=SC2029
    preflight_output=$(ssh "$SSH_HOST" "bash -s" "$REMOTE_PATH" << 'REMOTEEOF'
REPO_PATH="${1:-$HOME}"
if command -v podman &>/dev/null; then
    echo "RUNTIME=podman"
elif command -v docker &>/dev/null; then
    echo "RUNTIME=docker"
else
    echo "RUNTIME="
fi
if (command -v podman &>/dev/null && podman compose version &>/dev/null) || \
   (command -v docker &>/dev/null && docker compose version &>/dev/null); then
    echo "COMPOSE_AVAILABLE=1"
else
    echo "COMPOSE_AVAILABLE=0"
fi
if [ -d "$REPO_PATH" ]; then
    echo "REPO_PATH_EXISTS=1"
else
    echo "REPO_PATH_EXISTS=0"
fi
if [ -d "$REPO_PATH/.devcontainer" ]; then
    echo "DEVCONTAINER_EXISTS=1"
else
    echo "DEVCONTAINER_EXISTS=0"
fi
AVAIL_GB=$(df -BG "$REPO_PATH" 2>/dev/null | awk 'NR==2 {gsub(/G/,""); print $4}')
echo "DISK_AVAILABLE_GB=${AVAIL_GB:-0}"
if [ "$(uname -s)" = "Darwin" ]; then
    echo "OS_TYPE=macos"
else
    echo "OS_TYPE=linux"
fi
REMOTEEOF
    )

    while IFS= read -r line; do
        [[ "$line" =~ ^([A-Z_]+)=(.*)$ ]] || continue
        case "${BASH_REMATCH[1]}" in
            RUNTIME) RUNTIME="${BASH_REMATCH[2]}" ;;
            COMPOSE_AVAILABLE) COMPOSE_AVAILABLE="${BASH_REMATCH[2]}" ;;
            REPO_PATH_EXISTS) REPO_PATH_EXISTS="${BASH_REMATCH[2]}" ;;
            DEVCONTAINER_EXISTS) DEVCONTAINER_EXISTS="${BASH_REMATCH[2]}" ;;
            DISK_AVAILABLE_GB) DISK_AVAILABLE_GB="${BASH_REMATCH[2]}" ;;
            OS_TYPE) OS_TYPE="${BASH_REMATCH[2]}" ;;
        esac
    done <<< "$preflight_output"

    if [[ -z "${RUNTIME:-}" ]]; then
        log_error "No container runtime found on $SSH_HOST. Install podman or docker."
        exit 1
    fi
    if [[ "$RUNTIME" == "podman" ]]; then
        COMPOSE_CMD="podman compose"
    else
        COMPOSE_CMD="docker compose"
    fi
    if [[ "${COMPOSE_AVAILABLE:-0}" != "1" ]]; then
        log_error "Compose not available on $SSH_HOST. Install docker-compose or podman-compose."
        exit 1
    fi
    if [[ "${REPO_PATH_EXISTS:-0}" != "1" ]]; then
        log_error "Repository not found at $REMOTE_PATH on $SSH_HOST."
        exit 1
    fi
    if [[ "${DEVCONTAINER_EXISTS:-0}" != "1" ]]; then
        log_error "No .devcontainer/ found in $REMOTE_PATH. Is this a devcontainer-enabled project?"
        exit 1
    fi
    if [[ "${DISK_AVAILABLE_GB:-0}" -lt 2 ]] 2>/dev/null; then
        log_warning "Low disk space on $SSH_HOST (${DISK_AVAILABLE_GB:-0}GB). At least 2GB recommended."
    fi
    if [[ "${OS_TYPE:-}" == "macos" ]]; then
        log_warning "Remote host is macOS. Devcontainer support may be limited."
    fi
}

remote_compose_up() {
    local ps_output state health
    # shellcheck disable=SC2029
    ps_output=$(ssh "$SSH_HOST" "cd $REMOTE_PATH && $COMPOSE_CMD ps --format json 2>/dev/null" || true)
    state=$(echo "$ps_output" | grep -o '"State":"[^"]*"' | head -1 | cut -d'"' -f4)
    health=$(echo "$ps_output" | grep -o '"Health":"[^"]*"' | head -1 | cut -d'"' -f4)

    if [[ "$state" == "running" && "${health:-}" == "healthy" ]]; then
        log_success "Devcontainer already running on $SSH_HOST. Opening..."
    else
        log_info "Starting devcontainer on $SSH_HOST..."
        # shellcheck disable=SC2029
        if ! ssh "$SSH_HOST" "cd $REMOTE_PATH && $COMPOSE_CMD up -d"; then
            log_error "Failed to start devcontainer on $SSH_HOST."
            log_error "Debug with: ssh $SSH_HOST 'cd $REMOTE_PATH && $COMPOSE_CMD logs'"
            exit 1
        fi
        sleep 2
    fi
}

open_editor() {
    local container_workspace uri
    # Read workspaceFolder from devcontainer.json on remote host
    # shellcheck disable=SC2029
    container_workspace=$(ssh "$SSH_HOST" \
        "grep -o '\"workspaceFolder\"[[:space:]]*:[[:space:]]*\"[^\"]*\"' \
         ${REMOTE_PATH}/.devcontainer/devcontainer.json 2>/dev/null" \
        | sed 's/.*: *"//;s/"//' || echo "/workspace")

    # Default to /workspace if workspaceFolder not found
    container_workspace="${container_workspace:-/workspace}"

    # Build URI using Python helper
    uri=$(python3 "$SCRIPT_DIR/devc_remote_uri.py" \
        "$REMOTE_PATH" \
        "$SSH_HOST" \
        "$container_workspace")

    "$EDITOR_CLI" --folder-uri "$uri"
}

# ═══════════════════════════════════════════════════════════════════════════════
# MAIN
# ═══════════════════════════════════════════════════════════════════════════════

main() {
    parse_args "$@"

    log_info "Detecting local editor CLI..."
    detect_editor_cli
    log_success "Using $EDITOR_CLI"

    log_info "Checking SSH connectivity to $SSH_HOST..."
    check_ssh
    log_success "SSH connection OK"

    log_info "Running pre-flight checks on $SSH_HOST..."
    remote_preflight
    log_success "Pre-flight OK (runtime: $RUNTIME)"

    remote_compose_up
    open_editor

    log_success "Done — opened $EDITOR_CLI for $SSH_HOST:$REMOTE_PATH"
}

main "$@"
