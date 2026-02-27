"""Tests for domain models."""

from scitadel.domain.models import (
    Assessment,
    CandidatePaper,
    Paper,
    ResearchQuestion,
    Search,
    SearchResult,
    SearchTerm,
    SourceOutcome,
    SourceStatus,
)


class TestPaper:
    def test_create_paper_minimal(self):
        paper = Paper(title="Test Paper")
        assert paper.title == "Test Paper"
        assert paper.id  # auto-generated
        assert paper.authors == []
        assert paper.doi is None

    def test_create_paper_full(self):
        paper = Paper(
            title="Full Paper",
            authors=["Alice", "Bob"],
            abstract="An abstract.",
            doi="10.1234/test",
            year=2025,
            journal="Nature",
        )
        assert paper.doi == "10.1234/test"
        assert len(paper.authors) == 2
        assert paper.year == 2025


class TestCandidatePaper:
    def test_create_candidate(self):
        candidate = CandidatePaper(
            source="pubmed",
            source_id="12345",
            title="Candidate Paper",
            doi="10.1234/candidate",
            rank=1,
        )
        assert candidate.source == "pubmed"
        assert candidate.source_id == "12345"
        assert candidate.rank == 1


class TestSearch:
    def test_create_search(self):
        search = Search(
            query="PET tracer radiopharma",
            sources=["pubmed", "arxiv"],
        )
        assert search.id
        assert search.query == "PET tracer radiopharma"
        assert search.sources == ["pubmed", "arxiv"]

    def test_search_with_outcomes(self):
        search = Search(
            query="test",
            sources=["pubmed"],
            source_outcomes=[
                SourceOutcome(
                    source="pubmed",
                    status=SourceStatus.SUCCESS,
                    result_count=42,
                    latency_ms=150.0,
                ),
            ],
        )
        assert search.source_outcomes[0].status == SourceStatus.SUCCESS
        assert search.source_outcomes[0].result_count == 42


class TestSearchResult:
    def test_create_result(self):
        result = SearchResult(
            search_id="search-1",
            paper_id="paper-1",
            source="arxiv",
            rank=3,
            score=0.95,
        )
        assert result.search_id == "search-1"
        assert result.source == "arxiv"


class TestResearchQuestion:
    def test_create_question(self):
        q = ResearchQuestion(
            text="What PET tracers are used in oncology?",
            description="Focus on FDA-approved tracers.",
        )
        assert q.id
        assert "PET tracers" in q.text


class TestSearchTerm:
    def test_create_term(self):
        term = SearchTerm(
            question_id="q-1",
            terms=["PET", "tracer", "oncology"],
            query_string="PET tracer oncology",
        )
        assert term.question_id == "q-1"
        assert len(term.terms) == 3


class TestAssessment:
    def test_create_assessment(self):
        assessment = Assessment(
            paper_id="p-1",
            question_id="q-1",
            score=0.87,
            reasoning="Highly relevant: discusses PET tracer development.",
            model="claude-sonnet-4-6",
            assessor="claude-sonnet-4-6",
        )
        assert assessment.score == 0.87
        assert assessment.paper_id == "p-1"
