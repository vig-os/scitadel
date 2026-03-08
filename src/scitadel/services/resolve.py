"""Shared prefix-matching resolution for entity IDs.

Extracts the duplicated prefix-matching pattern used across
mcp_server.py, cli.py, and tui/data.py into a single function.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import TypeVar

T = TypeVar("T")


def resolve_prefix(items: list[T], prefix: str, get_id: Callable[[T], str]) -> T | None:
    """Resolve a prefix to a single item, or None if 0 or >1 matches.

    Args:
        items: List of items to search
        prefix: ID prefix to match
        get_id: Function to extract ID from an item

    Returns:
        The unique matching item, or None if ambiguous/not found.
    """
    # Try exact match first
    for item in items:
        if get_id(item) == prefix:
            return item

    # Fall back to prefix match
    matches = [item for item in items if get_id(item).startswith(prefix)]
    return matches[0] if len(matches) == 1 else None
