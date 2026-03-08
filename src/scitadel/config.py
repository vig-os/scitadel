"""Application configuration with env + file defaults."""

from __future__ import annotations

import os
import subprocess
from dataclasses import dataclass, field
from pathlib import Path


def _find_workspace_root() -> Path | None:
    """Find the workspace root by looking for a git repo from cwd upward."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0:
            return Path(result.stdout.strip())
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return None


def _default_db_path() -> Path:
    # 1. Explicit env var always wins
    env_db = os.environ.get("SCITADEL_DB")
    if env_db:
        return Path(env_db).expanduser()

    # 2. Workspace-local: .scitadel/scitadel.db in git root or cwd
    workspace = _find_workspace_root() or Path.cwd()
    return workspace / ".scitadel" / "scitadel.db"


@dataclass(frozen=True)
class SourceConfig:
    """Per-source adapter configuration."""

    enabled: bool = True
    timeout: float = 30.0
    max_retries: int = 3
    api_key: str = ""


@dataclass(frozen=True)
class ChatConfig:
    """Configuration for the chat-driven research assistant."""

    model: str = "claude-sonnet-4-6"
    max_tokens: int = 4096
    scoring_concurrency: int = 5


@dataclass(frozen=True)
class Config:
    """Top-level application configuration."""

    db_path: Path = field(default_factory=_default_db_path)
    default_sources: tuple[str, ...] = ("pubmed", "arxiv", "openalex", "inspire")
    pubmed: SourceConfig = field(default_factory=SourceConfig)
    arxiv: SourceConfig = field(default_factory=SourceConfig)
    openalex: SourceConfig = field(default_factory=SourceConfig)
    inspire: SourceConfig = field(default_factory=SourceConfig)
    chat: ChatConfig = field(default_factory=ChatConfig)


def load_config() -> Config:
    """Load configuration from environment variables with sensible defaults."""
    db_path = _default_db_path()

    pubmed = SourceConfig(
        api_key=os.environ.get("SCITADEL_PUBMED_API_KEY", ""),
    )
    openalex = SourceConfig(
        api_key=os.environ.get("SCITADEL_OPENALEX_EMAIL", ""),
    )
    chat = ChatConfig(
        model=os.environ.get("SCITADEL_CHAT_MODEL", "claude-sonnet-4-6"),
        max_tokens=int(os.environ.get("SCITADEL_CHAT_MAX_TOKENS", "4096")),
        scoring_concurrency=int(os.environ.get("SCITADEL_SCORING_CONCURRENCY", "5")),
    )

    return Config(
        db_path=db_path,
        pubmed=pubmed,
        openalex=openalex,
        chat=chat,
    )
