"""Deterministic deduplication and canonicalization engine.

DOI-first exact match with fuzzy-title fallback.
Provenance retained for merged records.
"""

from __future__ import annotations

import re
import unicodedata

from scitadel.domain.models import CandidatePaper, Paper, SearchResult


def _normalize_title(title: str) -> str:
    """Normalize title for fuzzy matching: lowercase, strip punctuation/whitespace."""
    title = unicodedata.normalize("NFKD", title)
    title = title.lower()
    title = re.sub(r"[^\w\s]", "", title)
    title = re.sub(r"\s+", " ", title).strip()
    return title


def _title_similarity(a: str, b: str) -> float:
    """Simple word-overlap similarity (Jaccard) for title matching."""
    words_a = set(_normalize_title(a).split())
    words_b = set(_normalize_title(b).split())
    if not words_a or not words_b:
        return 0.0
    intersection = words_a & words_b
    union = words_a | words_b
    return len(intersection) / len(union)


def _merge_candidate_into_paper(paper: Paper, candidate: CandidatePaper) -> Paper:
    """Merge a candidate's metadata into an existing paper (fill gaps)."""
    updates = {}

    if not paper.doi and candidate.doi:
        updates["doi"] = candidate.doi
    if not paper.arxiv_id and candidate.arxiv_id:
        updates["arxiv_id"] = candidate.arxiv_id
    if not paper.pubmed_id and candidate.pubmed_id:
        updates["pubmed_id"] = candidate.pubmed_id
    if not paper.inspire_id and candidate.inspire_id:
        updates["inspire_id"] = candidate.inspire_id
    if not paper.openalex_id and candidate.openalex_id:
        updates["openalex_id"] = candidate.openalex_id
    if not paper.abstract and candidate.abstract:
        updates["abstract"] = candidate.abstract
    if not paper.year and candidate.year:
        updates["year"] = candidate.year
    if not paper.journal and candidate.journal:
        updates["journal"] = candidate.journal
    if not paper.authors and candidate.authors:
        updates["authors"] = candidate.authors

    source_urls = dict(paper.source_urls)
    if candidate.url:
        source_urls[candidate.source] = candidate.url
    updates["source_urls"] = source_urls

    if updates:
        return paper.model_copy(update=updates)
    return paper


def deduplicate(
    candidates: list[CandidatePaper],
    title_threshold: float = 0.85,
) -> tuple[list[Paper], list[SearchResult]]:
    """Deduplicate candidates into canonical Papers.

    Returns:
        (papers, search_results) — deduplicated papers and per-source
        search result records with provenance.
    """
    doi_index: dict[str, int] = {}  # doi -> index in papers
    title_index: dict[str, int] = {}  # normalized title -> index in papers
    papers: list[Paper] = []
    search_results: list[SearchResult] = []

    for candidate in candidates:
        matched_idx = None

        # 1. DOI exact match
        if candidate.doi:
            doi_lower = candidate.doi.lower()
            if doi_lower in doi_index:
                matched_idx = doi_index[doi_lower]

        # 2. Fuzzy title match (only if no DOI match)
        if matched_idx is None and candidate.title:
            norm_title = _normalize_title(candidate.title)
            if norm_title in title_index:
                matched_idx = title_index[norm_title]
            else:
                # Check similarity against all existing titles
                for existing_title, idx in title_index.items():
                    if (
                        _title_similarity(candidate.title, papers[idx].title)
                        >= title_threshold
                    ):
                        matched_idx = idx
                        break

        if matched_idx is not None:
            # Merge into existing paper
            papers[matched_idx] = _merge_candidate_into_paper(
                papers[matched_idx], candidate
            )
        else:
            # Create new paper
            paper = Paper(
                title=candidate.title,
                authors=candidate.authors,
                abstract=candidate.abstract,
                doi=candidate.doi,
                arxiv_id=candidate.arxiv_id,
                pubmed_id=candidate.pubmed_id,
                inspire_id=candidate.inspire_id,
                openalex_id=candidate.openalex_id,
                year=candidate.year,
                journal=candidate.journal,
                url=candidate.url,
                source_urls={candidate.source: candidate.url} if candidate.url else {},
            )
            matched_idx = len(papers)
            papers.append(paper)

            if candidate.doi:
                doi_index[candidate.doi.lower()] = matched_idx
            if candidate.title:
                title_index[_normalize_title(candidate.title)] = matched_idx

        # Record search result provenance
        search_results.append(
            SearchResult(
                search_id="",  # filled by caller
                paper_id=papers[matched_idx].id,
                source=candidate.source,
                rank=candidate.rank,
                score=candidate.score,
                raw_metadata=candidate.raw_data,
            )
        )

    return papers, search_results
