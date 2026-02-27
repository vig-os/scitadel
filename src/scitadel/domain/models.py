"""Core domain models.

These models represent the canonical entities in the Scitadel system.
A paper's relevance is not a property of the paper — it's a property of the
paper in the context of a specific research question.
"""

from __future__ import annotations

import enum
import uuid
from datetime import datetime, timezone

from pydantic import BaseModel, Field


def _utcnow() -> datetime:
    return datetime.now(timezone.utc)


def _new_id() -> str:
    return uuid.uuid4().hex


class Paper(BaseModel):
    """Canonical, deduplicated paper record.

    A paper exists once regardless of how many searches found it.
    """

    id: str = Field(default_factory=_new_id)
    title: str
    authors: list[str] = Field(default_factory=list)
    abstract: str = ""
    doi: str | None = None
    arxiv_id: str | None = None
    pubmed_id: str | None = None
    inspire_id: str | None = None
    openalex_id: str | None = None
    year: int | None = None
    journal: str | None = None
    url: str | None = None
    source_urls: dict[str, str] = Field(default_factory=dict)
    created_at: datetime = Field(default_factory=_utcnow)
    updated_at: datetime = Field(default_factory=_utcnow)


class CandidatePaper(BaseModel):
    """Un-deduplicated paper record from a single source adapter.

    Adapters produce candidates; the dedup engine merges them into Papers.
    """

    source: str
    source_id: str
    title: str
    authors: list[str] = Field(default_factory=list)
    abstract: str = ""
    doi: str | None = None
    arxiv_id: str | None = None
    pubmed_id: str | None = None
    inspire_id: str | None = None
    openalex_id: str | None = None
    year: int | None = None
    journal: str | None = None
    url: str | None = None
    rank: int | None = None
    score: float | None = None
    raw_data: dict = Field(default_factory=dict)


class SourceStatus(str, enum.Enum):
    """Outcome status for a single source in a search run."""

    SUCCESS = "success"
    PARTIAL = "partial"
    FAILED = "failed"
    SKIPPED = "skipped"


class SourceOutcome(BaseModel):
    """Per-source result metadata for a search run."""

    source: str
    status: SourceStatus
    result_count: int = 0
    latency_ms: float = 0.0
    error: str | None = None


class Search(BaseModel):
    """Immutable search run record."""

    id: str = Field(default_factory=_new_id)
    query: str
    sources: list[str] = Field(default_factory=list)
    parameters: dict = Field(default_factory=dict)
    source_outcomes: list[SourceOutcome] = Field(default_factory=list)
    total_candidates: int = 0
    total_papers: int = 0
    created_at: datetime = Field(default_factory=_utcnow)


class SearchResult(BaseModel):
    """Join record: search -> paper, with per-source rank/score."""

    search_id: str
    paper_id: str
    source: str
    rank: int | None = None
    score: float | None = None
    raw_metadata: dict = Field(default_factory=dict)


class ResearchQuestion(BaseModel):
    """First-class research question entity."""

    id: str = Field(default_factory=_new_id)
    text: str
    description: str = ""
    created_at: datetime = Field(default_factory=_utcnow)
    updated_at: datetime = Field(default_factory=_utcnow)


class SearchTerm(BaseModel):
    """Keyword combination linked to a research question."""

    id: str = Field(default_factory=_new_id)
    question_id: str
    terms: list[str] = Field(default_factory=list)
    query_string: str = ""
    created_at: datetime = Field(default_factory=_utcnow)


class Assessment(BaseModel):
    """Paper x research question -> relevance score + provenance.

    Multiple assessments per paper (different questions, models, human override).
    """

    id: str = Field(default_factory=_new_id)
    paper_id: str
    question_id: str
    score: float
    reasoning: str = ""
    model: str | None = None
    prompt: str | None = None
    temperature: float | None = None
    assessor: str = ""  # "human", model name, etc.
    created_at: datetime = Field(default_factory=_utcnow)
