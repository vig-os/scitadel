"""Federated search orchestrator with partial-failure tolerance.

Executes selected adapters in parallel, applies retry/backoff,
and records per-source outcomes.
"""

from __future__ import annotations

import asyncio
import logging
import random
import time

from scitadel.adapters.base import SourceAdapter
from scitadel.domain.models import (
    CandidatePaper,
    Search,
    SourceOutcome,
    SourceStatus,
)

logger = logging.getLogger(__name__)


async def _run_adapter(
    adapter: SourceAdapter,
    query: str,
    max_results: int,
    max_retries: int = 3,
    search_id: str | None = None,
) -> tuple[list[CandidatePaper], SourceOutcome]:
    """Run a single adapter with retry logic. Never raises."""
    extra = {"search_id": search_id or ""}
    start = time.monotonic()
    last_error = ""

    for attempt in range(max_retries):
        try:
            candidates = await adapter.search(query, max_results=max_results)
            elapsed_ms = (time.monotonic() - start) * 1000
            logger.info(
                "Adapter %s returned %d results in %.0fms",
                adapter.name,
                len(candidates),
                elapsed_ms,
                extra=extra,
            )
            return candidates, SourceOutcome(
                source=adapter.name,
                status=SourceStatus.SUCCESS,
                result_count=len(candidates),
                latency_ms=elapsed_ms,
            )
        except Exception as exc:
            last_error = str(exc)
            logger.warning(
                "Adapter %s attempt %d/%d failed: %s",
                adapter.name,
                attempt + 1,
                max_retries,
                last_error,
                extra=extra,
            )
            if attempt < max_retries - 1:
                delay = 2**attempt * random.uniform(0.5, 1.5)
                await asyncio.sleep(delay)

    elapsed_ms = (time.monotonic() - start) * 1000
    return [], SourceOutcome(
        source=adapter.name,
        status=SourceStatus.FAILED,
        result_count=0,
        latency_ms=elapsed_ms,
        error=last_error,
    )


async def run_search(
    query: str,
    adapters: list[SourceAdapter],
    max_results: int = 50,
    max_retries: int = 3,
) -> tuple[Search, list[CandidatePaper]]:
    """Execute a federated search across all adapters in parallel.

    Returns (Search record, all CandidatePapers from all sources).
    Source failures do not abort the whole search.
    """
    # Pre-generate search ID for log correlation
    search = Search(
        query=query,
        sources=[a.name for a in adapters],
        parameters={"max_results": max_results},
        source_outcomes=[],
        total_candidates=0,
    )
    search_id = search.id

    logger.info("Starting search %s: query=%r, sources=%s", search_id[:8], query, [a.name for a in adapters])

    tasks = [
        _run_adapter(adapter, query, max_results, max_retries, search_id=search_id)
        for adapter in adapters
    ]
    results = await asyncio.gather(*tasks)

    all_candidates: list[CandidatePaper] = []
    outcomes: list[SourceOutcome] = []

    for candidates, outcome in results:
        all_candidates.extend(candidates)
        outcomes.append(outcome)

    search = search.model_copy(
        update={
            "source_outcomes": outcomes,
            "total_candidates": len(all_candidates),
        }
    )

    logger.info("Search %s complete: %d candidates from %d sources", search_id[:8], len(all_candidates), len(adapters))

    return search, all_candidates
