#!/usr/bin/env python3
"""Strip AI agent identity trailers from commit message before validation.

Runs in prepare-commit-msg stage. Reads COMMIT_EDITMSG, removes lines matching
trailer patterns from .github/agent-blocklist.toml, writes back.

Refs: #163
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


def _load_trailer_patterns(blocklist_path: Path) -> list[re.Pattern[str]]:
    """Load trailer regex patterns from blocklist TOML."""
    import tomllib

    with blocklist_path.open("rb") as f:
        data = tomllib.load(f)
    patterns = data.get("patterns", {}).get("trailers", [])
    return [re.compile(p) for p in patterns]


def strip_trailers(msg_path: Path, blocklist_path: Path) -> bool:
    """Remove lines matching trailer patterns. Returns True if any line was removed."""
    patterns = _load_trailer_patterns(blocklist_path)
    content = msg_path.read_text(encoding="utf-8", errors="replace")
    lines = content.splitlines(keepends=True)
    new_lines: list[str] = []
    changed = False
    for line in lines:
        stripped = line.rstrip("\n\r")
        if any(p.match(stripped) for p in patterns):
            changed = True
            continue
        new_lines.append(line)
    if changed:
        msg_path.write_text("".join(new_lines), encoding="utf-8")
    return changed


def main() -> int:
    """Entry point. Expects COMMIT_EDITMSG path as first arg."""
    if len(sys.argv) < 2:
        print(
            "Usage: prepare-commit-msg-strip-trailers <path-to-COMMIT_EDITMSG>",
            file=sys.stderr,
        )
        return 2
    msg_path = Path(sys.argv[1])
    project_root = Path(__file__).resolve().parent.parent
    blocklist_path = project_root / ".github" / "agent-blocklist.toml"
    if not blocklist_path.exists():
        return 0  # No blocklist, nothing to strip
    if not msg_path.exists():
        print(f"File not found: {msg_path}", file=sys.stderr)
        return 2
    strip_trailers(msg_path, blocklist_path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
