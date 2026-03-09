"""Tests for application services: orchestrator, dedup, export, chat engine."""

from __future__ import annotations

import asyncio
import json


from scitadel.domain.models import CandidatePaper, Paper, SourceStatus
from scitadel.services.chat_engine import _trim_messages
from scitadel.services.dedup import _normalize_title, _title_similarity, deduplicate
from scitadel.services.export import export_bibtex, export_csv, export_json
from scitadel.services.orchestrator import run_search


# -- Message trimming tests --


class TestTrimMessages:
    def test_no_trim_when_under_limit(self):
        msgs = [{"role": "user", "content": "hi"}, {"role": "assistant", "content": "hello"}]
        assert _trim_messages(msgs, 10) == msgs

    def test_trim_to_limit(self):
        msgs = [{"role": "user", "content": f"msg{i}"} for i in range(10)]
        result = _trim_messages(msgs, 4)
        assert len(result) == 4
        assert result[0]["content"] == "msg6"

    def test_preserves_tool_pair(self):
        msgs = [
            {"role": "user", "content": "msg0"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "let me search"},
                {"type": "tool_use", "id": "t1", "name": "search", "input": {}},
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "results"},
            ]},
            {"role": "assistant", "content": [{"type": "text", "text": "done"}]},
        ]
        # Trim to 2: would get last 2 messages (tool_result user + assistant)
        # But tool_result needs its preceding assistant, so we get 3
        result = _trim_messages(msgs, 2)
        assert len(result) == 3
        assert result[0]["role"] == "assistant"
        assert any(
            c.get("type") == "tool_use"
            for c in result[0]["content"]
        )

    def test_no_expansion_for_normal_user_message(self):
        msgs = [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "response"},
            {"role": "user", "content": "second"},
            {"role": "assistant", "content": "response2"},
        ]
        result = _trim_messages(msgs, 2)
        assert len(result) == 2


# -- Dedup tests --


class TestNormalization:
    def test_normalize_title(self):
        assert _normalize_title("  Hello,  World!  ") == "hello world"

    def test_normalize_unicode(self):
        assert _normalize_title("café") == "cafe"

    def test_title_similarity_identical(self):
        assert _title_similarity("PET Tracer Study", "PET Tracer Study") == 1.0

    def test_title_similarity_different(self):
        assert _title_similarity("Apples", "Oranges") == 0.0

    def test_title_similarity_partial(self):
        sim = _title_similarity("PET Tracer Study", "PET Tracer Analysis")
        assert 0.3 < sim < 0.8


class TestDedup:
    def test_doi_dedup(self):
        candidates = [
            CandidatePaper(
                source="pubmed",
                source_id="pm1",
                title="Same Paper",
                doi="10.1234/same",
            ),
            CandidatePaper(
                source="openalex",
                source_id="oa1",
                title="Same Paper (OpenAlex)",
                doi="10.1234/same",
                year=2024,
            ),
        ]
        papers, results = deduplicate(candidates)
        assert len(papers) == 1
        assert len(results) == 2
        assert papers[0].year == 2024  # merged from openalex

    def test_title_dedup(self):
        candidates = [
            CandidatePaper(
                source="pubmed",
                source_id="pm2",
                title="PET Tracer Development for Oncology",
            ),
            CandidatePaper(
                source="arxiv",
                source_id="ax1",
                title="PET Tracer Development for Oncology",
                arxiv_id="2301.00001",
            ),
        ]
        papers, results = deduplicate(candidates)
        assert len(papers) == 1
        assert papers[0].arxiv_id == "2301.00001"

    def test_different_papers_not_merged(self):
        candidates = [
            CandidatePaper(
                source="pubmed",
                source_id="pm3",
                title="Paper About Cats",
            ),
            CandidatePaper(
                source="arxiv",
                source_id="ax2",
                title="Paper About Dogs",
            ),
        ]
        papers, results = deduplicate(candidates)
        assert len(papers) == 2

    def test_metadata_merge_fills_gaps(self):
        candidates = [
            CandidatePaper(
                source="pubmed",
                source_id="pm4",
                title="Merged Paper",
                doi="10.1234/merged",
                pubmed_id="pm4",
                journal="Nature",
            ),
            CandidatePaper(
                source="arxiv",
                source_id="ax3",
                title="Merged Paper",
                doi="10.1234/merged",
                arxiv_id="2301.99999",
                abstract="Great abstract.",
            ),
        ]
        papers, _ = deduplicate(candidates)
        assert len(papers) == 1
        p = papers[0]
        assert p.pubmed_id == "pm4"
        assert p.arxiv_id == "2301.99999"
        assert p.journal == "Nature"
        assert p.abstract == "Great abstract."

    def test_source_urls_tracked(self):
        candidates = [
            CandidatePaper(
                source="pubmed",
                source_id="pm5",
                title="Multi-Source Paper",
                doi="10.1234/multi",
                url="https://pubmed.ncbi.nlm.nih.gov/pm5/",
            ),
            CandidatePaper(
                source="arxiv",
                source_id="ax4",
                title="Multi-Source Paper",
                doi="10.1234/multi",
                url="https://arxiv.org/abs/2301.00002",
            ),
        ]
        papers, _ = deduplicate(candidates)
        assert "pubmed" in papers[0].source_urls
        assert "arxiv" in papers[0].source_urls


# -- Export tests --


class TestExportJSON:
    def test_export_json_basic(self):
        papers = [Paper(id="ej1", title="JSON Paper", year=2024)]
        result = export_json(papers)
        data = json.loads(result)
        assert len(data) == 1
        assert data[0]["title"] == "JSON Paper"

    def test_export_json_empty(self):
        assert export_json([]) == "[]"


class TestExportCSV:
    def test_export_csv_basic(self):
        papers = [
            Paper(
                id="ec1",
                title="CSV Paper",
                authors=["Alice", "Bob"],
                year=2024,
                doi="10.1234/csv",
            )
        ]
        result = export_csv(papers)
        lines = result.strip().split("\n")
        assert len(lines) == 2  # header + 1 row
        assert "CSV Paper" in lines[1]
        assert "Alice; Bob" in lines[1]

    def test_export_csv_header(self):
        result = export_csv([])
        assert result.startswith("id,title,authors")


class TestExportEdgeCases:
    def test_export_json_missing_fields(self):
        papers = [Paper(id="mf1", title="Minimal")]
        result = export_json(papers)
        data = json.loads(result)
        assert data[0]["title"] == "Minimal"
        assert data[0]["doi"] is None
        assert data[0]["year"] is None

    def test_export_csv_special_chars(self):
        papers = [
            Paper(
                id="sc1",
                title='Paper with "quotes" and, commas',
                authors=["O'Brien, Tim"],
            )
        ]
        result = export_csv(papers)
        lines = result.strip().split("\n")
        assert len(lines) == 2
        # CSV should properly escape the special characters
        assert "quotes" in lines[1]
        assert "O'Brien" in lines[1]

    def test_export_csv_empty(self):
        result = export_csv([])
        lines = result.strip().split("\n")
        assert len(lines) == 1  # header only

    def test_export_bibtex_missing_fields(self):
        papers = [Paper(id="bf1", title="Just A Title")]
        result = export_bibtex(papers)
        assert "title = {Just A Title}" in result


class TestExportBibTeX:
    def test_export_bibtex_basic(self):
        papers = [
            Paper(
                id="eb1",
                title="BibTeX Paper",
                authors=["Smith, John", "Doe, Jane"],
                year=2024,
                journal="Nature",
                doi="10.1234/bibtex",
            )
        ]
        result = export_bibtex(papers)
        assert "@article{smith2024bibtex" in result
        assert "title = {BibTeX Paper}" in result
        assert "Smith, John and Doe, Jane" in result
        assert "doi = {10.1234/bibtex}" in result

    def test_export_bibtex_empty(self):
        assert export_bibtex([]) == ""

    def test_export_bibtex_with_arxiv(self):
        papers = [
            Paper(
                id="eb2",
                title="arXiv Paper",
                authors=["Researcher, Alice"],
                arxiv_id="2301.12345",
            )
        ]
        result = export_bibtex(papers)
        assert "eprint = {2301.12345}" in result
        assert "archiveprefix = {arXiv}" in result


# -- Orchestrator tests --


class _MockAdapter:
    """Mock adapter for testing orchestrator."""

    def __init__(
        self,
        name: str,
        results: list[CandidatePaper] | None = None,
        error: Exception | None = None,
    ):
        self._name = name
        self._results = results or []
        self._error = error

    @property
    def name(self) -> str:
        return self._name

    async def search(self, query: str, max_results: int = 50, **params):
        if self._error:
            raise self._error
        return self._results


class TestOrchestrator:
    def test_basic_search(self):
        adapter = _MockAdapter(
            "mock",
            results=[CandidatePaper(source="mock", source_id="m1", title="Mock Paper")],
        )
        search, candidates = asyncio.run(
            run_search("test query", [adapter], max_results=10)
        )
        assert search.query == "test query"
        assert search.total_candidates == 1
        assert len(candidates) == 1
        assert search.source_outcomes[0].status == SourceStatus.SUCCESS

    def test_partial_failure(self):
        good = _MockAdapter(
            "good",
            results=[CandidatePaper(source="good", source_id="g1", title="Good Paper")],
        )
        bad = _MockAdapter("bad", error=ConnectionError("timeout"))

        search, candidates = asyncio.run(run_search("test", [good, bad], max_retries=1))
        assert len(candidates) == 1
        assert search.source_outcomes[0].status == SourceStatus.SUCCESS
        assert search.source_outcomes[1].status == SourceStatus.FAILED

    def test_all_sources_fail(self):
        bad1 = _MockAdapter("bad1", error=RuntimeError("fail"))
        bad2 = _MockAdapter("bad2", error=RuntimeError("fail"))

        search, candidates = asyncio.run(
            run_search("test", [bad1, bad2], max_retries=1)
        )
        assert len(candidates) == 0
        assert all(o.status == SourceStatus.FAILED for o in search.source_outcomes)

    def test_parallel_execution(self):
        adapters = [
            _MockAdapter(
                f"source{i}",
                results=[
                    CandidatePaper(
                        source=f"source{i}",
                        source_id=f"s{i}",
                        title=f"Paper {i}",
                    )
                ],
            )
            for i in range(4)
        ]
        search, candidates = asyncio.run(run_search("parallel test", adapters))
        assert search.total_candidates == 4
        assert len(search.source_outcomes) == 4
