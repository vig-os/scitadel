"""Sortable results table for the research assistant."""

from __future__ import annotations

import webbrowser

from textual.widgets import DataTable

from scitadel.domain.models import Paper


class ResultsTable(DataTable):
    """Enhanced DataTable for displaying search results with scores.

    Columns: Score, Title, Authors, Year, DOI, ID (hidden key)
    Keybindings: s=sort by score, n=sort by title, y=sort by year, o=open DOI
    """

    DEFAULT_CSS = """
    ResultsTable {
        height: 1fr;
        min-height: 6;
    }
    """

    BINDINGS = [
        ("s", "sort_score", "Sort by score"),
        ("n", "sort_title", "Sort by title"),
        ("y", "sort_year", "Sort by year"),
        ("o", "open_link", "Open link"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._papers: dict[str, _PaperRow] = {}

    def on_mount(self) -> None:
        self.add_columns("Score", "Title", "Authors", "Year", "DOI")
        self.cursor_type = "row"

    def add_paper(self, paper: Paper, score: float | None = None) -> None:
        """Add or update a paper row."""
        if paper.id in self._papers:
            self._update_existing(paper.id, score)
            return

        score_display = _format_score(score)
        authors = "; ".join(paper.authors[:2])
        if len(paper.authors) > 2:
            authors += " et al."

        row_key = self.add_row(
            score_display,
            paper.title[:60],
            authors,
            str(paper.year or ""),
            paper.doi or "",
            key=paper.id,
        )

        self._papers[paper.id] = _PaperRow(
            paper=paper,
            score=score,
            row_key=row_key,
        )

    def update_score(self, paper_id: str, score: float) -> None:
        """Update the score for an existing paper row."""
        if paper_id not in self._papers:
            return
        self._papers[paper_id].score = score
        row_key = self._papers[paper_id].row_key
        self.update_cell(row_key, "Score", _format_score(score))

    def _update_existing(self, paper_id: str, score: float | None) -> None:
        if score is not None:
            self.update_score(paper_id, score)

    def action_sort_score(self) -> None:
        self.sort("Score", reverse=True)

    def action_sort_title(self) -> None:
        self.sort("Title")

    def action_sort_year(self) -> None:
        self.sort("Year", reverse=True)

    def action_open_link(self) -> None:
        """Open the selected paper's DOI or URL in the browser."""
        if self.cursor_row is None:
            return
        row_key, _ = self.coordinate_to_cell_key(self.cursor_coordinate)
        paper_id = str(row_key)
        entry = self._papers.get(paper_id)
        if not entry:
            return

        paper = entry.paper
        url = None
        if paper.doi:
            url = f"https://doi.org/{paper.doi}"
        elif paper.url:
            url = paper.url

        if url:
            webbrowser.open(url)


class _PaperRow:
    """Tracks paper data for a table row."""

    __slots__ = ("paper", "score", "row_key")

    def __init__(self, paper: Paper, score: float | None, row_key: object) -> None:
        self.paper = paper
        self.score = score
        self.row_key = row_key


def _format_score(score: float | None) -> str:
    """Format a score with color indicator."""
    if score is None:
        return "[dim]--[/dim]"
    if score >= 0.7:
        return f"[green]{score:.2f}[/green]"
    if score >= 0.4:
        return f"[yellow]{score:.2f}[/yellow]"
    return f"[red]{score:.2f}[/red]"
