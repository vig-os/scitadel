"""Repository port interfaces (abstract contracts).

All DB access goes through these protocols. No raw SQL in application services.
Concrete implementations (SQLite, future Dolt) implement these contracts.
"""

from __future__ import annotations

from typing import Protocol

from scitadel.domain.models import (
    Assessment,
    Citation,
    Paper,
    ResearchQuestion,
    Search,
    SearchResult,
    SearchTerm,
    SnowballRun,
)


class PaperRepository(Protocol):
    """Port for paper persistence."""

    def save(self, paper: Paper) -> None: ...

    def save_many(self, papers: list[Paper]) -> None: ...

    def get(self, paper_id: str) -> Paper | None: ...

    def find_by_doi(self, doi: str) -> Paper | None: ...

    def find_by_title(self, title: str, threshold: float = 0.85) -> Paper | None: ...

    def list_all(self, limit: int = 100, offset: int = 0) -> list[Paper]: ...


class SearchRepository(Protocol):
    """Port for search run persistence."""

    def save(self, search: Search) -> None: ...

    def get(self, search_id: str) -> Search | None: ...

    def save_results(self, results: list[SearchResult]) -> None: ...

    def get_results(self, search_id: str) -> list[SearchResult]: ...

    def list_searches(self, limit: int = 20) -> list[Search]: ...

    def diff_searches(
        self, search_id_a: str, search_id_b: str
    ) -> tuple[list[str], list[str]]:
        """Return (added_paper_ids, removed_paper_ids) between two runs."""
        ...


class ResearchQuestionRepository(Protocol):
    """Port for research question and search term persistence."""

    def save_question(self, question: ResearchQuestion) -> None: ...

    def get_question(self, question_id: str) -> ResearchQuestion | None: ...

    def list_questions(self) -> list[ResearchQuestion]: ...

    def save_term(self, term: SearchTerm) -> None: ...

    def get_terms(self, question_id: str) -> list[SearchTerm]: ...


class AssessmentRepository(Protocol):
    """Port for relevance assessment persistence."""

    def save(self, assessment: Assessment) -> None: ...

    def get_for_paper(
        self, paper_id: str, question_id: str | None = None
    ) -> list[Assessment]: ...

    def get_for_question(self, question_id: str) -> list[Assessment]: ...


class CitationRepository(Protocol):
    """Port for citation edge and snowball run persistence."""

    def save(self, citation: Citation) -> None: ...

    def save_many(self, citations: list[Citation]) -> None: ...

    def get_references(self, paper_id: str) -> list[Citation]: ...

    def get_citations(self, paper_id: str) -> list[Citation]: ...

    def exists(
        self, source_paper_id: str, target_paper_id: str, direction: str
    ) -> bool: ...

    def save_snowball_run(self, run: SnowballRun) -> None: ...

    def get_snowball_run(self, run_id: str) -> SnowballRun | None: ...

    def list_snowball_runs(self, limit: int = 20) -> list[SnowballRun]: ...
