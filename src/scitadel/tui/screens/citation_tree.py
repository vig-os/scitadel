"""Citation tree — pushed screen showing citation chains."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.screen import Screen
from textual.widgets import Footer, Header, Static, Tree


class CitationTree(Screen):
    """Screen: tree view of citation chains from a paper."""

    BINDINGS = [("escape", "app.pop_screen", "Back")]

    def __init__(self, paper_id: str) -> None:
        super().__init__()
        self._paper_id = paper_id

    def compose(self) -> ComposeResult:
        yield Header()
        yield Static(id="citation-header")
        yield Tree("Citations", id="citation-tree")
        yield Footer()

    def on_mount(self) -> None:
        store = self.app.store
        paper = store.get_paper(self._paper_id)
        header = self.query_one("#citation-header", Static)

        if not paper:
            header.update("Paper not found.")
            return

        header.update(f"Citations for: {paper.title[:60]}")

        tree = self.query_one("#citation-tree", Tree)
        tree.root.expand()

        # References (papers this paper cites)
        refs = store.get_references(self._paper_id)
        if refs:
            refs_node = tree.root.add("References (cites)", expand=True)
            for citation in refs:
                target = store.get_paper(citation.target_paper_id)
                label = (
                    f"{target.title[:60]} ({target.year or 'N/A'})"
                    if target
                    else citation.target_paper_id[:8]
                )
                refs_node.add_leaf(label)
        else:
            tree.root.add_leaf("No references found")

        # Cited by (papers that cite this paper)
        cites = store.get_citations(self._paper_id)
        if cites:
            cites_node = tree.root.add("Cited By", expand=True)
            for citation in cites:
                source = store.get_paper(citation.source_paper_id)
                label = (
                    f"{source.title[:60]} ({source.year or 'N/A'})"
                    if source
                    else citation.source_paper_id[:8]
                )
                cites_node.add_leaf(label)
        else:
            tree.root.add_leaf("No citing papers found")
