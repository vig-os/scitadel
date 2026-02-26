#!/usr/bin/env python3
"""Build Cursor/VS Code nested authority URI for remote devcontainers."""

from __future__ import annotations

import argparse
import json


def hex_encode(s: str) -> str:
    """Hex-encode a string (UTF-8)."""
    return s.encode().hex()


def build_uri(
    workspace_path: str,
    devcontainer_path: str,
    ssh_host: str,
    container_workspace: str,
) -> str:
    """Build vscode-remote URI for dev-container over SSH.

    Format: vscode-remote://dev-container+{DC_HEX}@ssh-remote+{SSH_SPEC}/{container_workspace}
    """
    if not workspace_path:
        raise ValueError("workspace_path cannot be empty")
    if not devcontainer_path:
        raise ValueError("devcontainer_path cannot be empty")
    if not ssh_host:
        raise ValueError("ssh_host cannot be empty")
    if not container_workspace:
        raise ValueError("container_workspace cannot be empty")
    spec = {
        "settingType": "config",
        "workspacePath": workspace_path,
        "devcontainerPath": devcontainer_path,
    }
    dc_hex = hex_encode(json.dumps(spec, separators=(",", ":")))
    path = "/" + container_workspace.lstrip("/")
    return f"vscode-remote://dev-container+{dc_hex}@ssh-remote+{ssh_host}{path}"


def main() -> None:
    """CLI entry point."""
    parser = argparse.ArgumentParser(
        description="Build Cursor/VS Code URI for remote devcontainers"
    )
    parser.add_argument("workspace_path", help="Workspace path on the remote host")
    parser.add_argument("ssh_host", help="SSH host from ~/.ssh/config")
    parser.add_argument("container_workspace", help="Container workspace path")
    parser.add_argument(
        "--devcontainer-path",
        help="Path to devcontainer.json (default: {workspace_path}/.devcontainer/devcontainer.json)",
    )
    args = parser.parse_args()

    devcontainer_path = args.devcontainer_path or (
        f"{args.workspace_path.rstrip('/')}/.devcontainer/devcontainer.json"
    )
    uri = build_uri(
        workspace_path=args.workspace_path,
        devcontainer_path=devcontainer_path,
        ssh_host=args.ssh_host,
        container_workspace=args.container_workspace,
    )
    print(uri)


if __name__ == "__main__":
    main()
