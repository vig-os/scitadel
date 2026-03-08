"""Base adapter contract for source integrations."""

from __future__ import annotations

from typing import Protocol

from scitadel.domain.models import CandidatePaper


class SourceAdapter(Protocol):
    """Port contract for source adapters.

    Each adapter translates a canonical search request into
    source-specific syntax and normalizes responses into CandidatePaper records.
    """

    @property
    def name(self) -> str:
        """Source identifier (e.g., 'pubmed', 'arxiv')."""
        ...

    async def search(
        self,
        query: str,
        max_results: int = 50,
        **params: object,
    ) -> list[CandidatePaper]:
        """Execute a search and return normalized candidate records."""
        ...
