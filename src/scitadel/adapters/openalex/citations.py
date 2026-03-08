"""OpenAlex citation data fetcher for snowball chaining.

Uses PyAlex to fetch forward (cited_by) and backward (references) citations.
"""

from __future__ import annotations

import logging

import pyalex
from pyalex import Works

from scitadel.domain.models import Paper

logger = logging.getLogger(__name__)


class OpenAlexCitationFetcher:
    """Fetch citation data from OpenAlex."""

    def __init__(self, email: str = "") -> None:
        if email:
            pyalex.config.email = email

    async def fetch_references(self, paper: Paper) -> list[dict]:
        """Fetch papers referenced by this paper (backward snowball).

        Returns list of work dicts from OpenAlex.
        """
        openalex_id = self._resolve_openalex_id(paper)
        if not openalex_id:
            logger.warning("No OpenAlex ID for paper %s", paper.id[:8])
            return []

        try:
            work = Works()[openalex_id]
            referenced_ids = work.get("referenced_works", [])
            if not referenced_ids:
                return []

            # Fetch referenced works in batches
            return self._batch_fetch(referenced_ids)
        except Exception as exc:
            logger.warning("Failed to fetch references for %s: %s", paper.id[:8], exc)
            return []

    async def fetch_cited_by(self, paper: Paper) -> list[dict]:
        """Fetch papers that cite this paper (forward snowball).

        Returns list of work dicts from OpenAlex.
        """
        openalex_id = self._resolve_openalex_id(paper)
        if not openalex_id:
            logger.warning("No OpenAlex ID for paper %s", paper.id[:8])
            return []

        try:
            results = Works().filter(cites=openalex_id).get(per_page=50)
            return list(results)
        except Exception as exc:
            logger.warning("Failed to fetch cited_by for %s: %s", paper.id[:8], exc)
            return []

    def _resolve_openalex_id(self, paper: Paper) -> str | None:
        """Get OpenAlex ID, resolving via DOI if needed."""
        if paper.openalex_id:
            oa_id = paper.openalex_id
            if not oa_id.startswith("W"):
                oa_id = oa_id.rsplit("/", 1)[-1]
            return oa_id

        if paper.doi:
            try:
                work = Works()["doi:" + paper.doi]
                if work and work.get("id"):
                    return work["id"].rsplit("/", 1)[-1]
            except Exception:
                pass

        return None

    def _batch_fetch(self, openalex_ids: list[str], batch_size: int = 50) -> list[dict]:
        """Fetch works in batches by OpenAlex ID."""
        results = []
        for i in range(0, len(openalex_ids), batch_size):
            batch = openalex_ids[i : i + batch_size]
            # Extract short IDs
            short_ids = [url.rsplit("/", 1)[-1] for url in batch if url]
            if not short_ids:
                continue
            try:
                pipe_filter = "|".join(short_ids)
                works = Works().filter(openalex_id=pipe_filter).get(per_page=batch_size)
                results.extend(works)
            except Exception as exc:
                logger.warning("Batch fetch failed: %s", exc)
        return results


def work_to_paper_dict(work: dict) -> dict:
    """Convert an OpenAlex work dict to Paper constructor kwargs."""
    openalex_id = work.get("id", "")
    short_id = openalex_id.rsplit("/", 1)[-1] if openalex_id else ""

    title = work.get("title") or work.get("display_name") or ""

    authors = []
    for authorship in work.get("authorships", []):
        author = authorship.get("author", {})
        name = author.get("display_name", "")
        if name:
            authors.append(name)

    abstract = ""
    abstract_index = work.get("abstract_inverted_index")
    if abstract_index:
        word_positions = []
        for word, positions in abstract_index.items():
            for pos in positions:
                word_positions.append((pos, word))
        word_positions.sort()
        abstract = " ".join(w for _, w in word_positions)

    doi_url = work.get("doi") or ""
    doi = doi_url.replace("https://doi.org/", "") if doi_url else None

    year = work.get("publication_year")

    journal = None
    primary_location = work.get("primary_location") or {}
    source = primary_location.get("source") or {}
    if source:
        journal = source.get("display_name")

    ids = work.get("ids", {})
    pmid_url = ids.get("pmid") or ""
    pmid = pmid_url.rsplit("/", 1)[-1] if pmid_url else None

    return {
        "title": title,
        "authors": authors,
        "abstract": abstract,
        "doi": doi,
        "openalex_id": short_id,
        "pubmed_id": pmid,
        "year": year,
        "journal": journal,
        "url": openalex_id,
    }
