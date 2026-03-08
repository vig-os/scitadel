"""Tests for SQLite repository implementations."""

import pytest

from scitadel.domain.models import (
    Assessment,
    Paper,
    ResearchQuestion,
    Search,
    SearchResult,
    SearchTerm,
    SourceOutcome,
    SourceStatus,
)
from scitadel.repositories.sqlite import (
    Database,
    SQLiteAssessmentRepository,
    SQLitePaperRepository,
    SQLiteResearchQuestionRepository,
    SQLiteSearchRepository,
)


@pytest.fixture()
def db():
    database = Database(":memory:")
    database.migrate()
    yield database
    database.close()


@pytest.fixture()
def paper_repo(db):
    return SQLitePaperRepository(db)


@pytest.fixture()
def search_repo(db):
    return SQLiteSearchRepository(db)


@pytest.fixture()
def question_repo(db):
    return SQLiteResearchQuestionRepository(db)


@pytest.fixture()
def assessment_repo(db):
    return SQLiteAssessmentRepository(db)


class TestDatabase:
    def test_migrate_creates_tables(self, db):
        tables = db.conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table'"
        ).fetchall()
        table_names = {t["name"] for t in tables}
        assert "papers" in table_names
        assert "searches" in table_names
        assert "search_results" in table_names
        assert "research_questions" in table_names
        assert "search_terms" in table_names
        assert "assessments" in table_names

    def test_schema_version_recorded(self, db):
        row = db.conn.execute(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1"
        ).fetchone()
        assert row["version"] == 3


class TestPaperRepository:
    def test_save_and_get(self, paper_repo):
        paper = Paper(
            id="p1",
            title="Test Paper",
            authors=["Alice"],
            doi="10.1234/test",
        )
        paper_repo.save(paper)
        result = paper_repo.get("p1")
        assert result is not None
        assert result.title == "Test Paper"
        assert result.authors == ["Alice"]
        assert result.doi == "10.1234/test"

    def test_find_by_doi(self, paper_repo):
        paper = Paper(id="p2", title="DOI Paper", doi="10.5678/doi")
        paper_repo.save(paper)
        result = paper_repo.find_by_doi("10.5678/doi")
        assert result is not None
        assert result.id == "p2"

    def test_find_by_doi_not_found(self, paper_repo):
        assert paper_repo.find_by_doi("nonexistent") is None

    def test_find_by_title(self, paper_repo):
        paper = Paper(id="p3", title="Unique Title Here")
        paper_repo.save(paper)
        result = paper_repo.find_by_title("unique title here")
        assert result is not None
        assert result.id == "p3"

    def test_save_many(self, paper_repo):
        papers = [Paper(id=f"pm{i}", title=f"Paper {i}") for i in range(5)]
        paper_repo.save_many(papers)
        all_papers = paper_repo.list_all()
        assert len(all_papers) == 5

    def test_list_all_with_pagination(self, paper_repo):
        papers = [Paper(id=f"pl{i}", title=f"Paper {i}") for i in range(10)]
        paper_repo.save_many(papers)
        page1 = paper_repo.list_all(limit=5, offset=0)
        page2 = paper_repo.list_all(limit=5, offset=5)
        assert len(page1) == 5
        assert len(page2) == 5


class TestSearchRepository:
    def test_save_and_get(self, search_repo):
        search = Search(
            id="s1",
            query="PET tracer",
            sources=["pubmed", "arxiv"],
            source_outcomes=[
                SourceOutcome(
                    source="pubmed",
                    status=SourceStatus.SUCCESS,
                    result_count=10,
                    latency_ms=200.0,
                ),
            ],
            total_candidates=10,
            total_papers=8,
        )
        search_repo.save(search)
        result = search_repo.get("s1")
        assert result is not None
        assert result.query == "PET tracer"
        assert result.sources == ["pubmed", "arxiv"]
        assert len(result.source_outcomes) == 1
        assert result.source_outcomes[0].status == SourceStatus.SUCCESS

    def test_save_and_get_results(self, search_repo, paper_repo):
        paper = Paper(id="p-sr", title="Search Result Paper")
        paper_repo.save(paper)
        search = Search(id="s-sr", query="test", sources=["pubmed"])
        search_repo.save(search)

        results = [
            SearchResult(
                search_id="s-sr",
                paper_id="p-sr",
                source="pubmed",
                rank=1,
                score=0.95,
            ),
        ]
        search_repo.save_results(results)
        retrieved = search_repo.get_results("s-sr")
        assert len(retrieved) == 1
        assert retrieved[0].paper_id == "p-sr"
        assert retrieved[0].rank == 1

    def test_list_searches(self, search_repo):
        for i in range(3):
            search_repo.save(
                Search(id=f"ls{i}", query=f"query {i}", sources=["pubmed"])
            )
        searches = search_repo.list_searches(limit=10)
        assert len(searches) == 3

    def test_diff_searches(self, search_repo, paper_repo):
        for pid in ["d1", "d2", "d3"]:
            paper_repo.save(Paper(id=pid, title=f"Paper {pid}"))

        search_repo.save(Search(id="sa", query="q", sources=["pubmed"]))
        search_repo.save(Search(id="sb", query="q", sources=["pubmed"]))

        search_repo.save_results(
            [
                SearchResult(search_id="sa", paper_id="d1", source="pubmed"),
                SearchResult(search_id="sa", paper_id="d2", source="pubmed"),
            ]
        )
        search_repo.save_results(
            [
                SearchResult(search_id="sb", paper_id="d2", source="pubmed"),
                SearchResult(search_id="sb", paper_id="d3", source="pubmed"),
            ]
        )

        added, removed = search_repo.diff_searches("sa", "sb")
        assert added == ["d3"]
        assert removed == ["d1"]


class TestResearchQuestionRepository:
    def test_save_and_get_question(self, question_repo):
        q = ResearchQuestion(
            id="q1",
            text="What PET tracers are used?",
            description="Focus on oncology.",
        )
        question_repo.save_question(q)
        result = question_repo.get_question("q1")
        assert result is not None
        assert "PET tracers" in result.text

    def test_list_questions(self, question_repo):
        for i in range(3):
            question_repo.save_question(
                ResearchQuestion(id=f"lq{i}", text=f"Question {i}")
            )
        questions = question_repo.list_questions()
        assert len(questions) == 3

    def test_save_and_get_terms(self, question_repo):
        q = ResearchQuestion(id="qt", text="Test question")
        question_repo.save_question(q)

        term = SearchTerm(
            id="t1",
            question_id="qt",
            terms=["PET", "tracer"],
            query_string="PET tracer",
        )
        question_repo.save_term(term)
        terms = question_repo.get_terms("qt")
        assert len(terms) == 1
        assert terms[0].terms == ["PET", "tracer"]


class TestAssessmentRepository:
    def test_save_and_get(self, assessment_repo, paper_repo, question_repo):
        paper_repo.save(Paper(id="ap", title="Assessed Paper"))
        question_repo.save_question(ResearchQuestion(id="aq", text="Question"))

        assessment = Assessment(
            id="a1",
            paper_id="ap",
            question_id="aq",
            score=0.92,
            reasoning="Very relevant.",
            assessor="human",
        )
        assessment_repo.save(assessment)

        results = assessment_repo.get_for_paper("ap")
        assert len(results) == 1
        assert results[0].score == 0.92

    def test_get_for_paper_with_question(
        self, assessment_repo, paper_repo, question_repo
    ):
        paper_repo.save(Paper(id="ap2", title="Paper 2"))
        question_repo.save_question(ResearchQuestion(id="aq2", text="Q2"))
        question_repo.save_question(ResearchQuestion(id="aq3", text="Q3"))

        assessment_repo.save(
            Assessment(id="a2", paper_id="ap2", question_id="aq2", score=0.8)
        )
        assessment_repo.save(
            Assessment(id="a3", paper_id="ap2", question_id="aq3", score=0.3)
        )

        all_assessments = assessment_repo.get_for_paper("ap2")
        assert len(all_assessments) == 2

        filtered = assessment_repo.get_for_paper("ap2", question_id="aq2")
        assert len(filtered) == 1
        assert filtered[0].score == 0.8
