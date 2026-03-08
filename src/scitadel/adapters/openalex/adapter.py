"""OpenAlex source adapter via PyAlex.

Docs: https://docs.openalex.org/
Rate limit: 100k req/day (polite pool with email).
"""

from __future__ import annotations

import pyalex
from pyalex import Works

from scitadel.domain.models import CandidatePaper


class OpenAlexAdapter:
    """OpenAlex adapter using PyAlex."""

    def __init__(self, email: str = "", timeout: float = 30.0) -> None:
        if email:
            pyalex.config.email = email
        self._timeout = timeout

    @property
    def name(self) -> str:
        return "openalex"

    async def search(
        self,
        query: str,
        max_results: int = 50,
        **params: object,
    ) -> list[CandidatePaper]:
        """Search OpenAlex and return normalized candidate records.

        PyAlex is synchronous; we wrap it for the async interface.
        """
        results = Works().search(query).get(per_page=max_results)
        return [_work_to_candidate(work, rank) for rank, work in enumerate(results, 1)]


def _work_to_candidate(work: dict, rank: int) -> CandidatePaper:
    """Map an OpenAlex work dict to a CandidatePaper."""
    openalex_id = work.get("id", "")
    # Extract short ID from URL
    short_id = openalex_id.rsplit("/", 1)[-1] if openalex_id else ""

    title = work.get("title") or work.get("display_name") or ""

    # Authors
    authors = []
    for authorship in work.get("authorships", []):
        author = authorship.get("author", {})
        name = author.get("display_name", "")
        if name:
            authors.append(name)

    # Abstract (OpenAlex returns inverted index)
    abstract = ""
    abstract_index = work.get("abstract_inverted_index")
    if abstract_index:
        abstract = _reconstruct_abstract(abstract_index)

    doi_url = work.get("doi") or ""
    doi = doi_url.replace("https://doi.org/", "") if doi_url else None

    year = work.get("publication_year")

    # Journal
    journal = None
    primary_location = work.get("primary_location") or {}
    source = primary_location.get("source") or {}
    if source:
        journal = source.get("display_name")

    # IDs
    ids = work.get("ids", {})
    pmid_url = ids.get("pmid") or ""
    pmid = pmid_url.rsplit("/", 1)[-1] if pmid_url else None

    return CandidatePaper(
        source="openalex",
        source_id=short_id,
        title=title,
        authors=authors,
        abstract=abstract,
        doi=doi,
        openalex_id=short_id,
        pubmed_id=pmid,
        year=year,
        journal=journal,
        url=openalex_id,
        rank=rank,
        raw_data=work,
    )


def _reconstruct_abstract(inverted_index: dict) -> str:
    """Reconstruct abstract text from OpenAlex inverted index format."""
    if not inverted_index:
        return ""
    word_positions: list[tuple[int, str]] = []
    for word, positions in inverted_index.items():
        for pos in positions:
            word_positions.append((pos, word))
    word_positions.sort()
    return " ".join(w for _, w in word_positions)
