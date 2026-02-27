"""End-to-end integration tests for the full search pipeline.

Covers RFC-001 success criteria S1-S8 using in-memory DB
and mock adapters (no real API calls).
"""

from __future__ import annotations

import asyncio
import json

from click.testing import CliRunner

from scitadel.cli import cli
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
from scitadel.repositories.sqlite import (
    Database,
    SQLiteAssessmentRepository,
    SQLitePaperRepository,
    SQLiteResearchQuestionRepository,
    SQLiteSearchRepository,
)
from scitadel.services.dedup import deduplicate
from scitadel.services.export import export_bibtex, export_csv, export_json
from scitadel.services.orchestrator import run_search


# -- Helpers --


class _MockAdapter:
    def __init__(self, name: str, results: list[CandidatePaper]):
        self._name = name
        self._results = results

    @property
    def name(self) -> str:
        return self._name

    async def search(self, query: str, max_results: int = 50, **params):
        return self._results


def _make_db():
    db = Database(":memory:")
    db.migrate()
    return db


# -- S1: Federated search → dedup → persist --


class TestS1FederatedSearchPipeline:
    """S1: Run federated search, deduplicate, and persist to DB."""

    def _cross_listed_candidates(self) -> list[CandidatePaper]:
        """A paper that appears in both PubMed and arXiv (same DOI)."""
        return [
            CandidatePaper(
                source="pubmed",
                source_id="PM001",
                title="PET Tracer Development for Oncology",
                authors=["Smith, John", "Doe, Jane"],
                abstract="A study on PET tracers.",
                doi="10.1234/pet-tracer",
                pubmed_id="PM001",
                year=2024,
                journal="Nature Methods",
                url="https://pubmed.ncbi.nlm.nih.gov/PM001/",
                rank=1,
            ),
            CandidatePaper(
                source="arxiv",
                source_id="2401.00001",
                title="PET Tracer Development for Oncology",
                authors=["John Smith", "Jane Doe"],
                abstract="A study on PET tracers for cancer imaging.",
                doi="10.1234/pet-tracer",
                arxiv_id="2401.00001",
                url="https://arxiv.org/abs/2401.00001",
                rank=1,
            ),
            CandidatePaper(
                source="openalex",
                source_id="W999",
                title="Unrelated Machine Learning Paper",
                authors=["Alice, Bob"],
                doi="10.5678/ml-paper",
                openalex_id="W999",
                year=2023,
                rank=1,
            ),
        ]

    def test_full_pipeline(self):
        candidates = self._cross_listed_candidates()
        adapters = [
            _MockAdapter("pubmed", [candidates[0]]),
            _MockAdapter("arxiv", [candidates[1]]),
            _MockAdapter("openalex", [candidates[2]]),
        ]

        # Orchestrate
        search_record, all_candidates = asyncio.run(
            run_search("PET tracer", adapters, max_results=10)
        )
        assert search_record.total_candidates == 3
        assert len(all_candidates) == 3

        # Dedup
        papers, search_results = deduplicate(all_candidates)
        assert len(papers) == 2  # cross-listed paper merged
        search_record = search_record.model_copy(update={"total_papers": len(papers)})

        # Persist
        db = _make_db()
        paper_repo = SQLitePaperRepository(db)
        search_repo = SQLiteSearchRepository(db)

        paper_repo.save_many(papers)
        search_repo.save(search_record)
        for sr in search_results:
            sr.search_id = search_record.id
        search_repo.save_results(search_results)

        # Verify
        stored_papers = paper_repo.list_all()
        assert len(stored_papers) == 2

        stored_search = search_repo.get(search_record.id)
        assert stored_search is not None
        assert stored_search.query == "PET tracer"
        assert stored_search.total_papers == 2

        stored_results = search_repo.get_results(search_record.id)
        assert len(stored_results) == 3  # 3 candidates → 3 search_results

        db.close()

    def test_merged_paper_has_all_source_ids(self):
        candidates = self._cross_listed_candidates()
        papers, _ = deduplicate(candidates)
        merged = next(p for p in papers if p.doi == "10.1234/pet-tracer")
        assert merged.pubmed_id == "PM001"
        assert merged.arxiv_id == "2401.00001"
        assert "pubmed" in merged.source_urls
        assert "arxiv" in merged.source_urls


# -- S2: Re-run and diff --


class TestS2SearchDiff:
    """S2: Re-running a search stores a new result set; two runs can be diffed."""

    def test_diff_two_searches(self):
        db = _make_db()
        paper_repo = SQLitePaperRepository(db)
        search_repo = SQLiteSearchRepository(db)

        # Create shared and unique papers
        shared = Paper(id="shared1", title="Shared Paper", doi="10.1/shared")
        only_a = Paper(id="onlyA", title="Only in A", doi="10.1/a")
        only_b = Paper(id="onlyB", title="Only in B", doi="10.1/b")
        paper_repo.save_many([shared, only_a, only_b])

        # Search A
        search_a = Search(id="searchA", query="test", total_papers=2)
        search_repo.save(search_a)
        search_repo.save_results(
            [
                SearchResult(search_id="searchA", paper_id="shared1", source="pubmed"),
                SearchResult(search_id="searchA", paper_id="onlyA", source="pubmed"),
            ]
        )

        # Search B
        search_b = Search(id="searchB", query="test", total_papers=2)
        search_repo.save(search_b)
        search_repo.save_results(
            [
                SearchResult(search_id="searchB", paper_id="shared1", source="pubmed"),
                SearchResult(search_id="searchB", paper_id="onlyB", source="pubmed"),
            ]
        )

        added, removed = search_repo.diff_searches("searchA", "searchB")
        assert "onlyB" in added
        assert "onlyA" in removed
        assert "shared1" not in added
        assert "shared1" not in removed

        db.close()

    def test_search_parameters_are_auditable(self):
        db = _make_db()
        search_repo = SQLiteSearchRepository(db)

        search = Search(
            query="PET tracer radiopharma",
            sources=["pubmed", "arxiv"],
            parameters={"max_results": 50},
            source_outcomes=[
                SourceOutcome(
                    source="pubmed",
                    status=SourceStatus.SUCCESS,
                    result_count=10,
                    latency_ms=500.0,
                ),
            ],
        )
        search_repo.save(search)

        stored = search_repo.get(search.id)
        assert stored.query == "PET tracer radiopharma"
        assert stored.sources == ["pubmed", "arxiv"]
        assert stored.parameters == {"max_results": 50}
        assert stored.source_outcomes[0].result_count == 10
        assert stored.source_outcomes[0].latency_ms == 500.0

        db.close()


# -- S3: Export formats --


class TestS3ExportFormats:
    """S3: Results export to BibTeX, JSON, CSV with complete metadata."""

    def _sample_papers(self) -> list[Paper]:
        return [
            Paper(
                id="exp1",
                title="Export Test Paper",
                authors=["Smith, John", "Doe, Jane"],
                abstract="An abstract about PET.",
                doi="10.1234/export",
                arxiv_id="2401.99999",
                pubmed_id="EXP001",
                year=2024,
                journal="Nature",
                url="https://example.com/paper",
            ),
        ]

    def test_json_roundtrip(self):
        papers = self._sample_papers()
        result = export_json(papers)
        data = json.loads(result)
        assert len(data) == 1
        p = data[0]
        assert p["title"] == "Export Test Paper"
        assert p["doi"] == "10.1234/export"
        assert p["authors"] == ["Smith, John", "Doe, Jane"]
        assert p["year"] == 2024

    def test_csv_has_all_fields(self):
        papers = self._sample_papers()
        result = export_csv(papers)
        lines = result.strip().split("\n")
        header = lines[0]
        for field in [
            "id",
            "title",
            "authors",
            "year",
            "journal",
            "doi",
            "arxiv_id",
            "pubmed_id",
            "abstract",
            "url",
        ]:
            assert field in header
        assert "Export Test Paper" in lines[1]

    def test_bibtex_complete(self):
        papers = self._sample_papers()
        result = export_bibtex(papers)
        assert "@article{" in result
        assert "title = {Export Test Paper}" in result
        assert "doi = {10.1234/export}" in result
        assert "eprint = {2401.99999}" in result
        assert "archiveprefix = {arXiv}" in result
        assert "journal = {Nature}" in result

    def test_export_from_db(self):
        """Export reads from persisted records, not transient memory."""
        db = _make_db()
        paper_repo = SQLitePaperRepository(db)
        search_repo = SQLiteSearchRepository(db)

        papers = self._sample_papers()
        paper_repo.save_many(papers)

        search = Search(id="exp-search", query="export test")
        search_repo.save(search)
        search_repo.save_results(
            [
                SearchResult(search_id="exp-search", paper_id="exp1", source="pubmed"),
            ]
        )

        # Retrieve and export like the CLI does
        results = search_repo.get_results("exp-search")
        paper_ids = {r.paper_id for r in results}
        loaded = [p for pid in paper_ids if (p := paper_repo.get(pid))]

        assert len(loaded) == 1
        json_out = export_json(loaded)
        assert "Export Test Paper" in json_out
        db.close()


# -- S5: Cross-source dedup --


class TestS5CrossSourceDedup:
    """S5: Dedup correctly merges the same paper from multiple sources."""

    def test_three_source_merge(self):
        candidates = [
            CandidatePaper(
                source="pubmed",
                source_id="PM100",
                title="Multi-Source Paper",
                doi="10.1234/multi",
                pubmed_id="PM100",
                journal="EJNMMI",
            ),
            CandidatePaper(
                source="arxiv",
                source_id="2401.55555",
                title="Multi-Source Paper",
                doi="10.1234/multi",
                arxiv_id="2401.55555",
                abstract="Detailed abstract here.",
            ),
            CandidatePaper(
                source="openalex",
                source_id="W555",
                title="Multi-Source Paper",
                doi="10.1234/multi",
                openalex_id="W555",
                year=2024,
                url="https://openalex.org/W555",
            ),
        ]
        papers, results = deduplicate(candidates)
        assert len(papers) == 1
        assert len(results) == 3

        p = papers[0]
        assert p.pubmed_id == "PM100"
        assert p.arxiv_id == "2401.55555"
        assert p.openalex_id == "W555"
        assert p.journal == "EJNMMI"
        assert p.abstract == "Detailed abstract here."
        assert p.year == 2024


# -- S6: Search history --


class TestS6SearchHistory:
    """S6: Search history is queryable via CLI."""

    def test_history_shows_searches(self, tmp_path):
        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])

        # Can't easily do a real search without network,
        # but we can verify the history command works on an empty DB.
        result = runner.invoke(cli, ["history"], env={"SCITADEL_DB": str(db_path)})
        assert result.exit_code == 0
        assert "No search history" in result.output


# -- S7: Library API parity --


class TestS7LibraryAPI:
    """S7: Library API supports the same operations as the CLI."""

    def test_library_search(self):
        adapter = _MockAdapter(
            "mock",
            [CandidatePaper(source="mock", source_id="m1", title="API Paper")],
        )
        search, candidates = asyncio.run(run_search("test", [adapter], max_results=10))
        assert search.query == "test"
        assert len(candidates) == 1

    def test_library_dedup(self):
        candidates = [
            CandidatePaper(source="a", source_id="1", title="Same", doi="10.1/x"),
            CandidatePaper(source="b", source_id="2", title="Same", doi="10.1/x"),
        ]
        papers, results = deduplicate(candidates)
        assert len(papers) == 1

    def test_library_export(self):
        papers = [Paper(id="lib1", title="Lib Paper", year=2024)]
        assert "Lib Paper" in export_json(papers)
        assert "Lib Paper" in export_csv(papers)
        assert "Lib Paper" in export_bibtex(papers)

    def test_library_persistence(self):
        db = _make_db()
        repo = SQLitePaperRepository(db)
        paper = Paper(id="persist1", title="Persist Test")
        repo.save(paper)
        loaded = repo.get("persist1")
        assert loaded is not None
        assert loaded.title == "Persist Test"
        db.close()


# -- S8: Research questions as first-class entities --


class TestS8ResearchQuestions:
    """S8: Research questions and search terms are DB entities."""

    def test_question_lifecycle(self):
        db = _make_db()
        q_repo = SQLiteResearchQuestionRepository(db)
        a_repo = SQLiteAssessmentRepository(db)
        p_repo = SQLitePaperRepository(db)

        # Create research question
        question = ResearchQuestion(
            id="rq1",
            text="What PET tracers are used in oncology?",
            description="Focus on clinical trials post-2020.",
        )
        q_repo.save_question(question)

        # Link search terms
        term = SearchTerm(
            id="st1",
            question_id="rq1",
            terms=["PET", "tracer", "oncology"],
            query_string="PET tracer oncology",
        )
        q_repo.save_term(term)

        # Save a paper and assess it
        paper = Paper(id="p1", title="PET in Oncology")
        p_repo.save(paper)

        assessment = Assessment(
            id="a1",
            paper_id="p1",
            question_id="rq1",
            score=0.92,
            reasoning="Directly addresses PET tracer use in cancer.",
            assessor="human",
        )
        a_repo.save(assessment)

        # Verify everything is queryable
        stored_q = q_repo.get_question("rq1")
        assert stored_q.text == "What PET tracers are used in oncology?"

        stored_terms = q_repo.get_terms("rq1")
        assert len(stored_terms) == 1
        assert stored_terms[0].query_string == "PET tracer oncology"

        stored_a = a_repo.get_for_paper("p1", question_id="rq1")
        assert len(stored_a) == 1
        assert stored_a[0].score == 0.92

        question_assessments = a_repo.get_for_question("rq1")
        assert len(question_assessments) == 1

        db.close()


# -- Upsert regression (FK violation bug) --


class TestUpsertRegression:
    """Regression test for the INSERT OR REPLACE FK violation bug."""

    def test_save_paper_with_existing_doi_no_fk_error(self):
        """Saving a paper with an existing DOI must not violate FKs."""
        db = _make_db()
        paper_repo = SQLitePaperRepository(db)
        search_repo = SQLiteSearchRepository(db)

        # Save initial paper and link to a search
        paper1 = Paper(id="p1", title="Original", doi="10.1/same")
        paper_repo.save(paper1)

        search = Search(id="s1", query="test")
        search_repo.save(search)
        search_repo.save_results(
            [
                SearchResult(search_id="s1", paper_id="p1", source="pubmed"),
            ]
        )

        # Update the same paper (same ID) — should upsert, not delete+insert
        paper1_updated = Paper(
            id="p1", title="Updated Title", doi="10.1/same", year=2024
        )
        paper_repo.save(paper1_updated)

        loaded = paper_repo.get("p1")
        assert loaded.title == "Updated Title"
        assert loaded.year == 2024

        # Search results still intact
        results = search_repo.get_results("s1")
        assert len(results) == 1
        assert results[0].paper_id == "p1"

        db.close()

    def test_upsert_preserves_existing_metadata(self):
        """Upsert should keep existing non-null fields (COALESCE behavior)."""
        db = _make_db()
        paper_repo = SQLitePaperRepository(db)

        paper1 = Paper(
            id="p2",
            title="Paper With Journal",
            doi="10.1/coalesce",
            journal="Nature",
            year=2024,
        )
        paper_repo.save(paper1)

        # Save again without journal/year — should preserve original values
        paper2 = Paper(id="p2", title="Paper With Journal", doi="10.1/coalesce")
        paper_repo.save(paper2)

        loaded = paper_repo.get("p2")
        assert loaded.journal == "Nature"
        assert loaded.year == 2024

        db.close()
