"""Paper detail — pushed screen showing full paper metadata + assessments."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import VerticalScroll
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header, Markdown, Static


class PaperDetail(Screen):
    """Screen: full paper metadata, abstract, and assessments."""

    BINDINGS = [
        ("escape", "app.pop_screen", "Back"),
        ("c", "show_citations", "Citations"),
    ]

    def __init__(self, paper_id: str) -> None:
        super().__init__()
        self._paper_id = paper_id

    def compose(self) -> ComposeResult:
        yield Header()
        yield VerticalScroll(
            Static(id="paper-meta"),
            Markdown(id="paper-abstract"),
            Static("Assessments:", id="assess-header"),
            DataTable(id="assess-table"),
        )
        yield Footer()

    def on_mount(self) -> None:
        store = self.app.store
        paper = store.get_paper(self._paper_id)
        if not paper:
            self.query_one("#paper-meta", Static).update("Paper not found.")
            return

        authors = "; ".join(paper.authors)
        meta = (
            f"**{paper.title}**\n\n"
            f"Authors: {authors}\n"
            f"Year: {paper.year or 'N/A'}  |  "
            f"Journal: {paper.journal or 'N/A'}\n"
            f"DOI: {paper.doi or 'N/A'}  |  ID: {paper.id[:8]}"
        )
        self.query_one("#paper-meta", Static).update(meta)

        abstract_widget = self.query_one("#paper-abstract", Markdown)
        abstract_text = paper.abstract or "_No abstract available._"
        abstract_widget.update(f"### Abstract\n\n{abstract_text}")

        # Assessments
        table = self.query_one("#assess-table", DataTable)
        table.add_columns("Score", "Assessor", "Date", "Reasoning")

        assessments = store.get_assessments_for_paper(paper.id)
        for a in assessments:
            table.add_row(
                f"{a.score:.2f}",
                a.assessor[:20],
                f"{a.created_at:%Y-%m-%d %H:%M}",
                a.reasoning[:80],
            )

    def action_show_citations(self) -> None:
        from scitadel.tui.screens.citation_tree import CitationTree

        self.app.push_screen(CitationTree(self._paper_id))
