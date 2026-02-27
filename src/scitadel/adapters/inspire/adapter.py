"""INSPIRE-HEP source adapter using the INSPIRE REST API.

Docs: https://github.com/inspirehep/rest-api-doc
Rate limit: generous, CERN-maintained.
"""

from __future__ import annotations

import httpx

from scitadel.domain.models import CandidatePaper

INSPIRE_API_URL = "https://inspirehep.net/api/literature"


class InspireAdapter:
    """INSPIRE-HEP REST API adapter."""

    def __init__(self, timeout: float = 30.0) -> None:
        self._timeout = timeout

    @property
    def name(self) -> str:
        return "inspire"

    async def search(
        self,
        query: str,
        max_results: int = 50,
        **params: object,
    ) -> list[CandidatePaper]:
        """Search INSPIRE-HEP and return normalized candidate records."""
        async with httpx.AsyncClient(timeout=self._timeout) as client:
            search_params = {
                "q": query,
                "size": max_results,
                "sort": "mostrecent",
                "fields": "titles,authors,abstracts,dois,arxiv_eprints,"
                "publication_info,external_system_identifiers,urls",
            }
            resp = await client.get(INSPIRE_API_URL, params=search_params)
            resp.raise_for_status()
            data = resp.json()
            return _parse_inspire_results(data)


def _parse_inspire_results(data: dict) -> list[CandidatePaper]:
    """Parse INSPIRE API response into CandidatePaper records."""
    candidates = []
    hits = data.get("hits", {}).get("hits", [])

    for rank, hit in enumerate(hits, start=1):
        meta = hit.get("metadata", {})
        inspire_id = str(hit.get("id", ""))

        # Title
        titles = meta.get("titles", [])
        title = titles[0].get("title", "") if titles else ""

        # Authors
        authors = []
        for auth in meta.get("authors", []):
            name = auth.get("full_name", "")
            if name:
                authors.append(name)

        # Abstract
        abstracts = meta.get("abstracts", [])
        abstract = abstracts[0].get("value", "") if abstracts else ""

        # DOI
        dois = meta.get("dois", [])
        doi = dois[0].get("value") if dois else None

        # arXiv ID
        arxiv_eprints = meta.get("arxiv_eprints", [])
        arxiv_id = arxiv_eprints[0].get("value") if arxiv_eprints else None

        # Year from publication_info
        year = None
        pub_info = meta.get("publication_info", [])
        if pub_info:
            year_str = pub_info[0].get("year")
            if year_str:
                try:
                    year = int(year_str)
                except (ValueError, TypeError):
                    pass

        # Journal
        journal = None
        if pub_info:
            journal = pub_info[0].get("journal_title")

        candidates.append(
            CandidatePaper(
                source="inspire",
                source_id=inspire_id,
                title=title,
                authors=authors,
                abstract=abstract,
                doi=doi,
                arxiv_id=arxiv_id,
                inspire_id=inspire_id,
                year=year,
                journal=journal,
                url=f"https://inspirehep.net/literature/{inspire_id}",
                rank=rank,
            )
        )

    return candidates
