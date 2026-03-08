"""Search browser — tab widget showing past searches."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Vertical
from textual.widgets import DataTable, Static


class SearchBrowser(Vertical):
    """Tab content: list of past search runs."""

    DEFAULT_CSS = """
    SearchBrowser {
        height: 1fr;
    }
    SearchBrowser > Static {
        height: 1;
        background: $primary-darken-2;
        color: $text;
        padding: 0 1;
    }
    """

    def compose(self) -> ComposeResult:
        yield Static("Past Searches — [Enter] to view papers, [r] to refresh")
        yield DataTable(id="search-table")

    def on_mount(self) -> None:
        self._load_data()

    def _load_data(self) -> None:
        table = self.query_one("#search-table", DataTable)
        table.clear(columns=True)
        table.add_columns("ID", "Date", "Query", "Papers", "Sources")
        table.cursor_type = "row"

        store = self.app.store
        searches = store.list_searches(limit=50)
        for s in searches:
            ok = sum(1 for o in s.source_outcomes if o.status.value == "success")
            table.add_row(
                s.id[:8],
                f"{s.created_at:%Y-%m-%d %H:%M}",
                s.query[:50],
                str(s.total_papers),
                f"{ok}/{len(s.source_outcomes)}",
                key=s.id,
            )

    def key_r(self) -> None:
        self._load_data()

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        from scitadel.tui.screens.paper_browser import PaperBrowser

        search_id = str(event.row_key.value)
        self.app.push_screen(PaperBrowser(search_id))
