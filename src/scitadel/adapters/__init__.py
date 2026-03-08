"""Source adapters for external academic APIs."""

from __future__ import annotations

from scitadel.adapters.base import SourceAdapter


def build_adapters(
    source_list: list[str],
    *,
    pubmed_api_key: str = "",
    openalex_email: str = "",
) -> list[SourceAdapter]:
    """Build adapter instances for the given source names.

    Raises ValueError for unknown source names.
    """
    from scitadel.adapters.arxiv.adapter import ArxivAdapter
    from scitadel.adapters.inspire.adapter import InspireAdapter
    from scitadel.adapters.openalex.adapter import OpenAlexAdapter
    from scitadel.adapters.pubmed.adapter import PubMedAdapter

    adapter_map: dict[str, callable] = {
        "pubmed": lambda: PubMedAdapter(api_key=pubmed_api_key),
        "arxiv": lambda: ArxivAdapter(),
        "openalex": lambda: OpenAlexAdapter(email=openalex_email),
        "inspire": lambda: InspireAdapter(),
    }

    adapters: list[SourceAdapter] = []
    for name in source_list:
        factory = adapter_map.get(name)
        if factory is None:
            raise ValueError(f"Unknown source: {name}")
        adapters.append(factory())
    return adapters


__all__ = ["SourceAdapter", "build_adapters"]
