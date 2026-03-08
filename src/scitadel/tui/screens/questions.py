"""Questions panel — tab widget for research questions + terms."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import DataTable, Static


class QuestionsPanel(Vertical):
    """Tab content: research questions with their terms and assessment stats."""

    DEFAULT_CSS = """
    QuestionsPanel {
        height: 1fr;
    }
    QuestionsPanel > Static {
        height: 1;
        background: $primary-darken-2;
        color: $text;
        padding: 0 1;
    }
    #question-detail {
        height: auto;
        max-height: 12;
        padding: 1;
        border-top: solid $primary;
    }
    """

    BINDINGS = [
        ("enter", "open_question", "Open in Research"),
        ("r", "refresh", "Refresh"),
    ]

    def compose(self) -> ComposeResult:
        yield Static("Research Questions — [Enter] to open in Research, [r] to refresh")
        yield Horizontal(
            DataTable(id="question-table"),
        )
        yield Static(id="question-detail")

    def on_mount(self) -> None:
        self._load_data()

    def _load_data(self) -> None:
        table = self.query_one("#question-table", DataTable)
        table.clear(columns=True)
        table.add_columns("ID", "Created", "Question", "Terms", "Assessments")
        table.cursor_type = "row"

        store = self.app.store
        questions = store.list_questions()
        for q in questions:
            terms = store.get_terms(q.id)
            assessments = store.get_assessments_for_question(q.id)
            table.add_row(
                q.id[:8],
                f"{q.created_at:%Y-%m-%d}",
                q.text[:50],
                str(len(terms)),
                str(len(assessments)),
                key=q.id,
            )

    def action_refresh(self) -> None:
        self._load_data()

    def action_open_question(self) -> None:
        """Open the selected question in the Research tab."""
        table = self.query_one("#question-table", DataTable)
        if table.cursor_row is None:
            return
        row_key, _ = table.coordinate_to_cell_key(table.cursor_coordinate)
        question_id = str(row_key)

        from scitadel.tui.screens.research_assistant import ResearchAssistant

        # Switch to Research tab
        tc = self.app.query_one("TabbedContent")
        tc.active = "tab-research"

        # Open the question in the research assistant
        ra = self.app.query_one(ResearchAssistant)
        ra.open_question(question_id)

    def on_data_table_row_highlighted(self, event: DataTable.RowHighlighted) -> None:
        if event.row_key is None:
            return
        question_id = str(event.row_key.value)
        store = self.app.store
        q = store.get_question(question_id)
        if not q:
            return

        terms = store.get_terms(q.id)
        assessments = store.get_assessments_for_question(q.id)

        detail_parts = [f"Question: {q.text}"]
        if q.description:
            detail_parts.append(f"Description: {q.description}")
        if terms:
            for t in terms:
                detail_parts.append(f"  Terms: {t.terms}  Query: {t.query_string}")
        if assessments:
            scores = [a.score for a in assessments]
            avg = sum(scores) / len(scores)
            relevant = sum(1 for s in scores if s >= 0.6)
            detail_parts.append(
                f"Assessments: {len(assessments)} | "
                f"Avg: {avg:.2f} | Relevant: {relevant}"
            )

        self.query_one("#question-detail", Static).update("\n".join(detail_parts))
