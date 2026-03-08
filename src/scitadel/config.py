"""Application configuration with env + file defaults."""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path


def _default_db_path() -> Path:
    return Path(os.environ.get("SCITADEL_DB", "~/.scitadel/scitadel.db")).expanduser()


@dataclass(frozen=True)
class SourceConfig:
    """Per-source adapter configuration."""

    enabled: bool = True
    timeout: float = 30.0
    max_retries: int = 3
    api_key: str = ""


@dataclass(frozen=True)
class Config:
    """Top-level application configuration."""

    db_path: Path = field(default_factory=_default_db_path)
    default_sources: tuple[str, ...] = ("pubmed", "arxiv", "openalex", "inspire")
    pubmed: SourceConfig = field(default_factory=SourceConfig)
    arxiv: SourceConfig = field(default_factory=SourceConfig)
    openalex: SourceConfig = field(default_factory=SourceConfig)
    inspire: SourceConfig = field(default_factory=SourceConfig)


def load_config() -> Config:
    """Load configuration from environment variables with sensible defaults."""
    db_path = _default_db_path()

    pubmed = SourceConfig(
        api_key=os.environ.get("SCITADEL_PUBMED_API_KEY", ""),
    )
    openalex = SourceConfig(
        api_key=os.environ.get("SCITADEL_OPENALEX_EMAIL", ""),
    )

    return Config(
        db_path=db_path,
        pubmed=pubmed,
        openalex=openalex,
    )
