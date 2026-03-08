"""Paper browser — pushed screen showing papers from a search."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header, Static


class PaperBrowser(Screen):
    """Screen: papers from a specific search run."""

    BINDINGS = [("escape", "app.pop_screen", "Back")]

    def __init__(self, search_id: str) -> None:
        super().__init__()
        self._search_id = search_id

    def compose(self) -> ComposeResult:
        yield Header()
        yield Static(id="paper-header")
        yield DataTable(id="paper-table")
        yield Footer()

    def on_mount(self) -> None:
        store = self.app.store
        search = store.get_search(self._search_id)
        header = self.query_one("#paper-header", Static)
        if search:
            header.update(
                f'Search {search.id[:8]} — "{search.query[:50]}" — '
                f"{search.total_papers} papers"
            )
        else:
            header.update(f"Search {self._search_id[:8]}")

        table = self.query_one("#paper-table", DataTable)
        table.add_columns("ID", "Year", "Title", "Authors", "DOI")
        table.cursor_type = "row"

        papers = store.get_papers_for_search(self._search_id)
        for p in papers:
            authors = "; ".join(p.authors[:2])
            if len(p.authors) > 2:
                authors += " et al."
            table.add_row(
                p.id[:8],
                str(p.year or ""),
                p.title[:60],
                authors[:40],
                p.doi or "",
                key=p.id,
            )

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        from scitadel.tui.screens.paper_detail import PaperDetail

        paper_id = str(event.row_key.value)
        self.app.push_screen(PaperDetail(paper_id))
