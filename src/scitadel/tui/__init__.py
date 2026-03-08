"""Scitadel TUI — interactive terminal dashboard built with Textual."""

from __future__ import annotations


def main() -> None:
    """Launch the Scitadel TUI application."""
    from scitadel.tui.app import ScitadelApp

    app = ScitadelApp()
    app.run()


__all__ = ["main"]
