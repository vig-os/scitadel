"""Domain models and repository port interfaces."""

from scitadel.domain.models import (
    Assessment,
    CandidatePaper,
    Paper,
    ResearchQuestion,
    Search,
    SearchResult,
    SearchTerm,
    SourceOutcome,
    SourceStatus,
)
from scitadel.domain.ports import (
    AssessmentRepository,
    PaperRepository,
    ResearchQuestionRepository,
    SearchRepository,
)

__all__ = [
    "Assessment",
    "AssessmentRepository",
    "CandidatePaper",
    "Paper",
    "PaperRepository",
    "ResearchQuestion",
    "ResearchQuestionRepository",
    "Search",
    "SearchRepository",
    "SearchResult",
    "SearchTerm",
    "SourceOutcome",
    "SourceStatus",
]
