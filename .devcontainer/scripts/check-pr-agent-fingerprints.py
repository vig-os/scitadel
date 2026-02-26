#!/usr/bin/env python3
"""Check PR title and body for AI agent identity fingerprints.

Exits 1 if any blocklisted pattern is found. Used by pr-title-check CI.
Refs: #163
"""

from __future__ import annotations

import os
import sys
from pathlib import Path


def main() -> int:
    """Entry point. Reads PR_TITLE and PR_BODY from env."""
    title = os.environ.get("PR_TITLE", "")
    body = os.environ.get("PR_BODY", "")

    project_root = Path(__file__).resolve().parent.parent
    blocklist_path = project_root / ".github" / "agent-blocklist.toml"
    if not blocklist_path.exists():
        return 0

    from vig_utils.agent_blocklist import contains_agent_fingerprint, load_blocklist

    blocklist = load_blocklist(blocklist_path)
    content = f"{title}\n{body}"
    match = contains_agent_fingerprint(content, blocklist)
    if match:
        print(
            f"PR title or body contains blocked AI agent fingerprint: '{match}'. "
            "Remove agent identity from the PR.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
