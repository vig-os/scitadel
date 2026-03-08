#!/usr/bin/env python3
"""Reject commits when git author/committer matches AI agent identity blocklist.

Runs in pre-commit stage. Checks GIT_AUTHOR_NAME, GIT_AUTHOR_EMAIL,
GIT_COMMITTER_NAME, GIT_COMMITTER_EMAIL (from env) and git config user.name/email.
Skips when running in CI (GITHUB_ACTIONS=true or CI=true).

Refs: #163
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


def _get_git_config(cwd: Path, key: str) -> str:
    """Get git config value, empty string if not set."""
    try:
        result = subprocess.run(
            ["git", "config", key],
            cwd=cwd,
            capture_output=True,
            text=True,
            check=False,
        )
        return (result.stdout or "").strip()
    except Exception:
        return ""


def _check_value(value: str, blocklist: dict) -> str | None:
    """Return matching pattern if value matches blocklist, else None."""
    if not value:
        return None
    value_lower = value.lower()
    for name in blocklist.get("names", []):
        if name in value_lower:
            return name
    for email in blocklist.get("emails", []):
        if email in value_lower:
            return email
    return None


def main() -> int:
    """Entry point. Exits 1 if author/committer matches blocklist."""
    if os.environ.get("GITHUB_ACTIONS") == "true" or os.environ.get("CI") == "true":
        return 0  # Skip in CI; Dependabot etc. use bot identities

    project_root = Path(__file__).resolve().parent.parent
    blocklist_path = project_root / ".github" / "agent-blocklist.toml"
    if not blocklist_path.exists():
        return 0

    from vig_utils.agent_blocklist import load_blocklist

    blocklist = load_blocklist(blocklist_path)

    values_to_check: list[tuple[str, str]] = [
        ("GIT_AUTHOR_NAME", os.environ.get("GIT_AUTHOR_NAME", "")),
        ("GIT_AUTHOR_EMAIL", os.environ.get("GIT_AUTHOR_EMAIL", "")),
        ("GIT_COMMITTER_NAME", os.environ.get("GIT_COMMITTER_NAME", "")),
        ("GIT_COMMITTER_EMAIL", os.environ.get("GIT_COMMITTER_EMAIL", "")),
    ]
    # git config user.name/email when not in env (e.g. user-initiated commit)
    if not values_to_check[0][1]:
        values_to_check.append(
            ("user.name", _get_git_config(project_root, "user.name"))
        )
    if not values_to_check[1][1]:
        values_to_check.append(
            ("user.email", _get_git_config(project_root, "user.email"))
        )

    for label, value in values_to_check:
        match = _check_value(value, blocklist)
        if match:
            print(
                f"Git {label} matches blocklisted AI agent identity: '{match}'. "
                "Set author/committer to your own identity.",
                file=sys.stderr,
            )
            return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
