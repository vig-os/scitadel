"""Search terms bar widget — shows active keywords as chips."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal
from textual.widgets import Label, Static


class TermsBar(Static):
    """Horizontal bar showing active search terms/keywords as chips."""

    DEFAULT_CSS = """
    TermsBar {
        height: auto;
        min-height: 1;
        max-height: 3;
        padding: 0 1;
        background: $surface-darken-1;
    }
    TermsBar .term-chip {
        background: $primary;
        color: $text;
        padding: 0 1;
        margin: 0 1 0 0;
    }
    TermsBar #terms-label {
        color: $text-muted;
        padding: 0 1 0 0;
    }
    TermsBar #terms-container {
        height: auto;
    }
    """

    def __init__(self) -> None:
        super().__init__()
        self._terms: list[str] = []

    def compose(self) -> ComposeResult:
        with Horizontal(id="terms-container"):
            yield Label("Terms:", id="terms-label")

    def add_terms(self, terms: list[str]) -> None:
        """Add keyword chips to the bar."""
        container = self.query_one("#terms-container", Horizontal)
        for term in terms:
            normalized = term.strip().lower()
            if normalized and normalized not in self._terms:
                self._terms.append(normalized)
                chip = Label(term.strip(), classes="term-chip")
                container.mount(chip)

    def clear(self) -> None:
        """Remove all term chips."""
        container = self.query_one("#terms-container", Horizontal)
        for chip in container.query(".term-chip"):
            chip.remove()
        self._terms.clear()

    @property
    def terms(self) -> list[str]:
        return list(self._terms)
