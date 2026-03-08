"""arXiv source adapter using the arXiv API.

Docs: https://info.arxiv.org/help/api/index.html
Rate limit: be polite, ~1 req/3s recommended.
"""

from __future__ import annotations

import re

import defusedxml.ElementTree as ET
import httpx

from scitadel.domain.models import CandidatePaper

ARXIV_API_URL = "https://export.arxiv.org/api/query"

# Namespace used in arXiv Atom feed
NS = {"atom": "http://www.w3.org/2005/Atom", "arxiv": "http://arxiv.org/schemas/atom"}


class ArxivAdapter:
    """arXiv API adapter."""

    def __init__(self, timeout: float = 30.0) -> None:
        self._timeout = timeout

    @property
    def name(self) -> str:
        return "arxiv"

    async def search(
        self,
        query: str,
        max_results: int = 50,
        **params: object,
    ) -> list[CandidatePaper]:
        """Search arXiv and return normalized candidate records."""
        async with httpx.AsyncClient(timeout=self._timeout) as client:
            search_params = {
                "search_query": f"all:{query}",
                "start": 0,
                "max_results": max_results,
                "sortBy": "relevance",
                "sortOrder": "descending",
            }
            resp = await client.get(ARXIV_API_URL, params=search_params)
            resp.raise_for_status()
            return _parse_arxiv_atom(resp.text)


def _parse_arxiv_atom(xml_text: str) -> list[CandidatePaper]:
    """Parse arXiv Atom XML response into CandidatePaper records."""
    root = ET.fromstring(xml_text)
    candidates = []

    for rank, entry in enumerate(root.findall("atom:entry", NS), start=1):
        entry_id = entry.findtext("atom:id", "", NS)
        arxiv_id = _extract_arxiv_id(entry_id)

        title = entry.findtext("atom:title", "", NS)
        title = " ".join(title.split())  # collapse whitespace

        summary = entry.findtext("atom:summary", "", NS)
        abstract = " ".join(summary.split())

        authors = []
        for author in entry.findall("atom:author", NS):
            name = author.findtext("atom:name", "", NS)
            if name:
                authors.append(name)

        # DOI from arxiv:doi element
        doi_el = entry.find("arxiv:doi", NS)
        doi = doi_el.text if doi_el is not None else None

        # Published date -> year
        published = entry.findtext("atom:published", "", NS)
        year = None
        if published:
            try:
                year = int(published[:4])
            except ValueError:
                pass

        # Journal ref
        journal_el = entry.find("arxiv:journal_ref", NS)
        journal = journal_el.text if journal_el is not None else None

        candidates.append(
            CandidatePaper(
                source="arxiv",
                source_id=arxiv_id,
                title=title,
                authors=authors,
                abstract=abstract,
                doi=doi,
                arxiv_id=arxiv_id,
                year=year,
                journal=journal,
                url=entry_id,
                rank=rank,
            )
        )

    return candidates


def _extract_arxiv_id(url: str) -> str:
    """Extract arXiv ID from URL like http://arxiv.org/abs/2301.12345v1."""
    match = re.search(r"arxiv\.org/abs/(.+?)(?:v\d+)?$", url)
    return match.group(1) if match else url
