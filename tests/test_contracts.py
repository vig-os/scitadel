"""Adapter contract tests — hit real APIs to detect schema drift.

Run with: pytest -m contract
These are excluded from the default test run.
"""

from __future__ import annotations

import asyncio

import pytest

from scitadel.adapters.arxiv.adapter import ArxivAdapter
from scitadel.adapters.inspire.adapter import InspireAdapter
from scitadel.adapters.openalex.adapter import OpenAlexAdapter
from scitadel.adapters.pubmed.adapter import PubMedAdapter

# A known query that should return results on every source.
# "CRISPR" is broad enough for PubMed/arXiv/OpenAlex;
# "particle detector" covers INSPIRE.
PUBMED_QUERY = "CRISPR"
ARXIV_QUERY = "CRISPR"
OPENALEX_QUERY = "CRISPR"
INSPIRE_QUERY = "particle detector"


@pytest.mark.contract
class TestPubMedContract:
    def test_returns_results(self):
        adapter = PubMedAdapter()
        results = asyncio.run(adapter.search(PUBMED_QUERY, max_results=5))
        assert len(results) > 0, "PubMed returned no results — possible API change"

    def test_result_schema(self):
        adapter = PubMedAdapter()
        results = asyncio.run(adapter.search(PUBMED_QUERY, max_results=3))
        for r in results:
            assert r.source == "pubmed"
            assert r.source_id, "Missing source_id (PMID)"
            assert r.pubmed_id, "Missing pubmed_id"
            assert r.title, "Missing title"
            assert r.rank is not None

    def test_metadata_populated(self):
        adapter = PubMedAdapter()
        results = asyncio.run(adapter.search(PUBMED_QUERY, max_results=5))
        # At least some results should have DOIs and authors
        has_doi = any(r.doi for r in results)
        has_authors = any(r.authors for r in results)
        assert has_doi, "No results had DOIs — possible parsing issue"
        assert has_authors, "No results had authors — possible parsing issue"


@pytest.mark.contract
class TestArxivContract:
    def test_returns_results(self):
        adapter = ArxivAdapter()
        results = asyncio.run(adapter.search(ARXIV_QUERY, max_results=5))
        assert len(results) > 0, "arXiv returned no results — possible API change"

    def test_result_schema(self):
        adapter = ArxivAdapter()
        results = asyncio.run(adapter.search(ARXIV_QUERY, max_results=3))
        for r in results:
            assert r.source == "arxiv"
            assert r.arxiv_id, "Missing arxiv_id"
            assert r.title, "Missing title"
            assert r.url, "Missing url"

    def test_metadata_populated(self):
        adapter = ArxivAdapter()
        results = asyncio.run(adapter.search(ARXIV_QUERY, max_results=5))
        has_abstract = any(r.abstract for r in results)
        has_authors = any(r.authors for r in results)
        assert has_abstract, "No results had abstracts — possible parsing issue"
        assert has_authors, "No results had authors — possible parsing issue"


@pytest.mark.contract
class TestOpenAlexContract:
    def test_returns_results(self):
        adapter = OpenAlexAdapter()
        results = asyncio.run(adapter.search(OPENALEX_QUERY, max_results=5))
        assert len(results) > 0, "OpenAlex returned no results — possible API change"

    def test_result_schema(self):
        adapter = OpenAlexAdapter()
        results = asyncio.run(adapter.search(OPENALEX_QUERY, max_results=3))
        for r in results:
            assert r.source == "openalex"
            assert r.openalex_id, "Missing openalex_id"
            assert r.title, "Missing title"

    def test_metadata_populated(self):
        adapter = OpenAlexAdapter()
        results = asyncio.run(adapter.search(OPENALEX_QUERY, max_results=5))
        has_doi = any(r.doi for r in results)
        has_year = any(r.year for r in results)
        assert has_doi, "No results had DOIs — possible parsing issue"
        assert has_year, "No results had years — possible parsing issue"


@pytest.mark.contract
class TestInspireContract:
    def test_returns_results(self):
        adapter = InspireAdapter()
        results = asyncio.run(adapter.search(INSPIRE_QUERY, max_results=5))
        assert len(results) > 0, "INSPIRE returned no results — possible API change"

    def test_result_schema(self):
        adapter = InspireAdapter()
        results = asyncio.run(adapter.search(INSPIRE_QUERY, max_results=3))
        for r in results:
            assert r.source == "inspire"
            assert r.inspire_id, "Missing inspire_id"
            assert r.title, "Missing title"

    def test_metadata_populated(self):
        adapter = InspireAdapter()
        results = asyncio.run(adapter.search(INSPIRE_QUERY, max_results=5))
        has_authors = any(r.authors for r in results)
        assert has_authors, "No results had authors — possible parsing issue"


@pytest.mark.contract
class TestCrossSourceDedup:
    """Contract test: search a known cross-listed paper and verify dedup merges it."""

    def test_known_crosslisted_paper(self):
        """Search for a broad query on PubMed + OpenAlex and verify DOI-based dedup."""
        from scitadel.services.dedup import deduplicate

        pubmed = PubMedAdapter()
        openalex = OpenAlexAdapter()

        # Broad query with enough results to guarantee overlap
        pm_results = asyncio.run(pubmed.search("CRISPR Cas9", max_results=30))
        oa_results = asyncio.run(openalex.search("CRISPR Cas9", max_results=30))

        all_candidates = pm_results + oa_results
        papers, results = deduplicate(all_candidates)

        # With 30 results from each source on a broad query, there should be overlap
        assert len(papers) < len(all_candidates), (
            f"Expected dedup to merge some papers but got "
            f"{len(papers)} papers from {len(all_candidates)} candidates"
        )
