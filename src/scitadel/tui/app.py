"""Main Textual App for Scitadel TUI."""

from __future__ import annotations

from pathlib import Path

from textual.app import App, ComposeResult
from textual.widgets import Footer, Header, TabbedContent, TabPane

from scitadel.tui.data import DataStore
from scitadel.tui.screens.questions import QuestionsPanel
from scitadel.tui.screens.research_assistant import ResearchAssistant
from scitadel.tui.screens.search_browser import SearchBrowser

CSS_PATH = Path(__file__).parent / "styles" / "app.tcss"


class ScitadelApp(App):
    """Scitadel interactive TUI dashboard."""

    TITLE = "Scitadel"
    SUB_TITLE = "Scientific Literature Retrieval"
    CSS_PATH = CSS_PATH

    BINDINGS = [
        ("q", "quit", "Quit"),
        ("d", "toggle_dark", "Dark/Light"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self.store = DataStore()

    def compose(self) -> ComposeResult:
        yield Header()
        with TabbedContent():
            with TabPane("Searches", id="tab-searches"):
                yield SearchBrowser()
            with TabPane("Questions", id="tab-questions"):
                yield QuestionsPanel()
            with TabPane("Research", id="tab-research"):
                yield ResearchAssistant()
        yield Footer()

    def on_unmount(self) -> None:
        self.store.close()
