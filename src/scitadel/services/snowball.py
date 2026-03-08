"""Snowball (citation chaining) service.

Implements forward and backward citation chaining with relevance-gated expansion.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Protocol

from scitadel.domain.models import (
    Citation,
    CitationDirection,
    Paper,
    ResearchQuestion,
    SnowballRun,
)

logger = logging.getLogger(__name__)

MAX_DEPTH_HARD_CAP = 3


class CitationFetcher(Protocol):
    """Protocol for fetching citation data from external sources."""

    async def fetch_references(self, paper: Paper) -> list[dict]: ...

    async def fetch_cited_by(self, paper: Paper) -> list[dict]: ...


class PaperResolver(Protocol):
    """Protocol for resolving/persisting papers from citation data."""

    def resolve(self, work_dict: dict) -> tuple[Paper, bool]:
        """Resolve a work dict to a Paper, returning (paper, is_new)."""
        ...


class Scorer(Protocol):
    """Protocol for scoring a paper against a question."""

    def score(self, paper: Paper, question: ResearchQuestion) -> float: ...


@dataclass
class SnowballConfig:
    """Configuration for a snowball run."""

    direction: str = "both"  # "references", "cited_by", "both"
    max_depth: int = 1
    threshold: float = 0.6
    max_papers_per_level: int = 50


@dataclass
class SnowballContext:
    """Mutable state for a snowball run."""

    citations: list[Citation] = field(default_factory=list)
    discovered_papers: list[Paper] = field(default_factory=list)
    new_papers: list[Paper] = field(default_factory=list)
    seen_ids: set[str] = field(default_factory=set)


async def snowball(
    seed_papers: list[Paper],
    question: ResearchQuestion,
    fetcher: CitationFetcher,
    resolver: PaperResolver,
    scorer: Scorer | None = None,
    config: SnowballConfig | None = None,
    on_progress: callable | None = None,
) -> tuple[SnowballRun, list[Citation], list[Paper]]:
    """Run citation chaining from seed papers.

    Args:
        seed_papers: Papers to start snowballing from
        question: Research question for relevance scoring
        fetcher: Citation data fetcher
        resolver: Paper dedup/persist resolver
        scorer: Relevance scorer (None = save all, no gating)
        config: Snowball configuration
        on_progress: Optional callback(depth, paper, score, is_new)

    Returns:
        (SnowballRun summary, list of Citation edges, list of new Papers)
    """
    config = config or SnowballConfig()
    effective_depth = min(config.max_depth, MAX_DEPTH_HARD_CAP)

    ctx = SnowballContext()
    ctx.seen_ids = {p.id for p in seed_papers}

    frontier = list(seed_papers)

    for depth in range(1, effective_depth + 1):
        next_frontier: list[Paper] = []

        for paper in frontier:
            work_dicts: list[tuple[dict, CitationDirection]] = []

            if config.direction in ("references", "both"):
                refs = await fetcher.fetch_references(paper)
                work_dicts.extend(
                    (w, CitationDirection.REFERENCES) for w in refs
                )

            if config.direction in ("cited_by", "both"):
                cites = await fetcher.fetch_cited_by(paper)
                work_dicts.extend(
                    (w, CitationDirection.CITED_BY) for w in cites
                )

            for work_dict, direction in work_dicts[: config.max_papers_per_level]:
                resolved_paper, is_new = resolver.resolve(work_dict)

                if resolved_paper.id in ctx.seen_ids:
                    continue
                ctx.seen_ids.add(resolved_paper.id)

                # Record citation edge
                if direction == CitationDirection.REFERENCES:
                    citation = Citation(
                        source_paper_id=paper.id,
                        target_paper_id=resolved_paper.id,
                        direction=direction,
                        discovered_by="openalex",
                        depth=depth,
                    )
                else:
                    citation = Citation(
                        source_paper_id=resolved_paper.id,
                        target_paper_id=paper.id,
                        direction=direction,
                        discovered_by="openalex",
                        depth=depth,
                    )
                ctx.citations.append(citation)
                ctx.discovered_papers.append(resolved_paper)

                if is_new:
                    ctx.new_papers.append(resolved_paper)

                # Score and gate (if scorer provided)
                if scorer is not None:
                    try:
                        score = scorer.score(resolved_paper, question)
                    except Exception as exc:
                        logger.warning(
                            "Scoring failed for %s: %s",
                            resolved_paper.id[:8],
                            exc,
                        )
                        score = 0.0

                    if on_progress:
                        on_progress(depth, resolved_paper, score, is_new)

                    if score >= config.threshold:
                        next_frontier.append(resolved_paper)
                else:
                    # No scorer — expand all discovered papers
                    if on_progress:
                        on_progress(depth, resolved_paper, 0.0, is_new)
                    next_frontier.append(resolved_paper)

        frontier = next_frontier
        if not frontier:
            break

    run = SnowballRun(
        question_id=question.id,
        direction=config.direction,
        max_depth=effective_depth,
        threshold=config.threshold,
        total_discovered=len(ctx.discovered_papers),
        total_new_papers=len(ctx.new_papers),
    )

    return run, ctx.citations, ctx.new_papers
