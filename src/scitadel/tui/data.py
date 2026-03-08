"""DataStore — thin wrapper over repositories for TUI screens."""

from __future__ import annotations

from scitadel.config import load_config
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
from scitadel.repositories.sqlite import (
    Database,
    SQLiteAssessmentRepository,
    SQLiteCitationRepository,
    SQLitePaperRepository,
    SQLiteResearchQuestionRepository,
    SQLiteSearchRepository,
)
from scitadel.services.resolve import resolve_prefix


class DataStore:
    """Single DB lifecycle wrapper providing typed repo accessors."""

    def __init__(self) -> None:
        config = load_config()
        self._db = Database(config.db_path)
        self._db.migrate()
        self._papers = SQLitePaperRepository(self._db)
        self._searches = SQLiteSearchRepository(self._db)
        self._questions = SQLiteResearchQuestionRepository(self._db)
        self._assessments = SQLiteAssessmentRepository(self._db)
        self._citations = SQLiteCitationRepository(self._db)

    def close(self) -> None:
        self._db.close()

    # -- Papers --

    def get_paper(self, paper_id: str) -> Paper | None:
        return self._papers.get(paper_id)

    def list_papers(self, limit: int = 100, offset: int = 0) -> list[Paper]:
        return self._papers.list_all(limit=limit, offset=offset)

    # -- Searches --

    def list_searches(self, limit: int = 50) -> list[Search]:
        return self._searches.list_searches(limit=limit)

    def get_search(self, search_id: str) -> Search | None:
        return self._searches.get(search_id)

    def get_search_results(self, search_id: str) -> list[SearchResult]:
        return self._searches.get_results(search_id)

    def get_papers_for_search(self, search_id: str) -> list[Paper]:
        results = self._searches.get_results(search_id)
        paper_ids = {r.paper_id for r in results}
        return [p for pid in paper_ids if (p := self._papers.get(pid))]

    # -- Questions --

    def list_questions(self) -> list[ResearchQuestion]:
        return self._questions.list_questions()

    def get_question(self, question_id: str) -> ResearchQuestion | None:
        return self._questions.get_question(question_id)

    def get_terms(self, question_id: str) -> list[SearchTerm]:
        return self._questions.get_terms(question_id)

    # -- Assessments --

    def get_assessments_for_paper(
        self, paper_id: str, question_id: str | None = None
    ) -> list[Assessment]:
        return self._assessments.get_for_paper(paper_id, question_id=question_id)

    def get_assessments_for_question(self, question_id: str) -> list[Assessment]:
        return self._assessments.get_for_question(question_id)

    # -- Citations --

    def get_references(self, paper_id: str) -> list[Citation]:
        return self._citations.get_references(paper_id)

    def get_citations(self, paper_id: str) -> list[Citation]:
        return self._citations.get_citations(paper_id)

    def list_snowball_runs(self, limit: int = 20) -> list[SnowballRun]:
        return self._citations.list_snowball_runs(limit=limit)

    # -- Write methods (used by ToolDispatcher) --

    def save_paper(self, paper: Paper) -> None:
        self._papers.save(paper)

    def save_papers(self, papers: list[Paper]) -> None:
        self._papers.save_many(papers)

    def save_search(self, search: Search) -> None:
        self._searches.save(search)

    def save_search_results(self, results: list[SearchResult]) -> None:
        self._searches.save_results(results)

    def save_question(self, question: ResearchQuestion) -> None:
        self._questions.save_question(question)

    def save_term(self, term: SearchTerm) -> None:
        self._questions.save_term(term)

    def save_assessment(self, assessment: Assessment) -> None:
        self._assessments.save(assessment)

    def find_paper_by_doi(self, doi: str) -> Paper | None:
        return self._papers.find_by_doi(doi)

    def find_paper_by_title(self, title: str) -> Paper | None:
        return self._papers.find_by_title(title)

    def resolve_prefix_id(self, entity_type: str, prefix: str) -> str | None:
        """Resolve a short ID prefix to a full ID. Returns None if ambiguous."""
        if entity_type == "search":
            items = self._searches.list_searches(limit=100)
        elif entity_type == "paper":
            items = self._papers.list_all(limit=1000)
        elif entity_type == "question":
            items = self._questions.list_questions()
        else:
            return None
        match = resolve_prefix(items, prefix, lambda x: x.id)
        return match.id if match else None

    def save_citations(self, citations: list[Citation]) -> None:
        self._citations.save_many(citations)

    def save_snowball_run(self, run: SnowballRun) -> None:
        self._citations.save_snowball_run(run)
