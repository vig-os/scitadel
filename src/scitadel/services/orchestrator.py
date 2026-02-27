"""Federated search orchestrator with partial-failure tolerance.

Executes selected adapters in parallel, applies retry/backoff,
and records per-source outcomes.
"""

from __future__ import annotations

import asyncio
import logging
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
) -> tuple[list[CandidatePaper], SourceOutcome]:
    """Run a single adapter with retry logic. Never raises."""
    start = time.monotonic()
    last_error = ""

    for attempt in range(max_retries):
        try:
            candidates = await adapter.search(query, max_results=max_results)
            elapsed_ms = (time.monotonic() - start) * 1000
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
            )
            if attempt < max_retries - 1:
                await asyncio.sleep(2**attempt)  # exponential backoff

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
    tasks = [
        _run_adapter(adapter, query, max_results, max_retries) for adapter in adapters
    ]
    results = await asyncio.gather(*tasks)

    all_candidates: list[CandidatePaper] = []
    outcomes: list[SourceOutcome] = []

    for candidates, outcome in results:
        all_candidates.extend(candidates)
        outcomes.append(outcome)

    search = Search(
        query=query,
        sources=[a.name for a in adapters],
        parameters={"max_results": max_results},
        source_outcomes=outcomes,
        total_candidates=len(all_candidates),
    )

    return search, all_candidates
