"""LLM-assisted relevance scoring service.

Two modes:
1. Direct API — calls Claude API via Anthropic SDK for batch scoring
2. Agent mode — spawns headless Claude CLI with Scitadel MCP tools
"""

from __future__ import annotations

import json
import logging
from collections.abc import Callable
from dataclasses import dataclass

import anthropic

from scitadel.domain.models import Assessment, Paper, ResearchQuestion

logger = logging.getLogger(__name__)

DEFAULT_MODEL = "claude-sonnet-4-6"
DEFAULT_TEMPERATURE = 0.0

SCORING_SYSTEM_PROMPT = """\
You are a scientific literature relevance assessor. You evaluate how relevant \
a paper is to a specific research question.

Score on a scale of 0.0 to 1.0:
- 0.0-0.2: Not relevant — different topic, no connection
- 0.2-0.4: Tangentially relevant — related field but doesn't address the question
- 0.4-0.6: Moderately relevant — partially addresses the question or related methodology
- 0.6-0.8: Relevant — directly addresses aspects of the question
- 0.8-1.0: Highly relevant — core paper for this research question

Respond with valid JSON only: {"score": float, "reasoning": "string"}
The reasoning should be 1-3 sentences explaining your assessment."""

SCORING_USER_PROMPT = """\
Research Question: {question_text}
{question_description}

Paper Title: {title}
Authors: {authors}
Year: {year}
Journal: {journal}
Abstract: {abstract}

Rate the relevance of this paper to the research question."""


@dataclass
class ScoringConfig:
    """Configuration for the scoring service."""

    model: str = DEFAULT_MODEL
    temperature: float = DEFAULT_TEMPERATURE
    max_tokens: int = 512


def _build_user_prompt(paper: Paper, question: ResearchQuestion) -> str:
    """Build the scoring prompt from paper and question data."""
    return SCORING_USER_PROMPT.format(
        question_text=question.text,
        question_description=(
            f"Context: {question.description}" if question.description else ""
        ),
        title=paper.title,
        authors="; ".join(paper.authors[:5]),
        year=paper.year or "N/A",
        journal=paper.journal or "N/A",
        abstract=paper.abstract[:2000] or "No abstract available.",
    )


def _build_assessment(
    paper: Paper,
    question: ResearchQuestion,
    config: ScoringConfig,
    user_prompt: str,
    raw_text: str,
) -> Assessment:
    """Parse API response and build an Assessment."""
    parsed = _parse_scoring_response(raw_text)
    return Assessment(
        paper_id=paper.id,
        question_id=question.id,
        score=parsed["score"],
        reasoning=parsed["reasoning"],
        model=config.model,
        prompt=user_prompt,
        temperature=config.temperature,
        assessor=config.model,
    )


def score_paper(
    paper: Paper,
    question: ResearchQuestion,
    config: ScoringConfig | None = None,
    client: anthropic.Anthropic | None = None,
) -> Assessment:
    """Score a single paper against a research question using Claude API.

    Returns an Assessment with full provenance.
    """
    config = config or ScoringConfig()
    client = client or anthropic.Anthropic()

    user_prompt = _build_user_prompt(paper, question)

    response = client.messages.create(
        model=config.model,
        max_tokens=config.max_tokens,
        temperature=config.temperature,
        system=SCORING_SYSTEM_PROMPT,
        messages=[{"role": "user", "content": user_prompt}],
    )

    return _build_assessment(paper, question, config, user_prompt, response.content[0].text)


async def score_paper_async(
    paper: Paper,
    question: ResearchQuestion,
    config: ScoringConfig | None = None,
    client: anthropic.AsyncAnthropic | None = None,
) -> Assessment:
    """Async version of score_paper using AsyncAnthropic.

    Returns an Assessment with full provenance.
    """
    config = config or ScoringConfig()
    client = client or anthropic.AsyncAnthropic()

    user_prompt = _build_user_prompt(paper, question)

    response = await client.messages.create(
        model=config.model,
        max_tokens=config.max_tokens,
        temperature=config.temperature,
        system=SCORING_SYSTEM_PROMPT,
        messages=[{"role": "user", "content": user_prompt}],
    )

    return _build_assessment(paper, question, config, user_prompt, response.content[0].text)


def score_papers(
    papers: list[Paper],
    question: ResearchQuestion,
    config: ScoringConfig | None = None,
    client: anthropic.Anthropic | None = None,
    on_progress: Callable | None = None,
) -> list[Assessment]:
    """Score multiple papers against a research question.

    Args:
        papers: Papers to score
        question: Research question to score against
        config: Scoring configuration
        client: Anthropic client (created if not provided)
        on_progress: Optional callback(index, total, paper, assessment)

    Returns:
        List of Assessments with full provenance.
    """
    config = config or ScoringConfig()
    client = client or anthropic.Anthropic()
    assessments = []

    for i, paper in enumerate(papers):
        try:
            assessment = score_paper(paper, question, config=config, client=client)
            assessments.append(assessment)
            logger.info(
                "Scored paper %d/%d: %.2f — %s",
                i + 1,
                len(papers),
                assessment.score,
                paper.title[:60],
            )
            if on_progress:
                on_progress(i, len(papers), paper, assessment)
        except Exception as exc:
            logger.warning(
                "Failed to score paper %d/%d (%s): %s",
                i + 1,
                len(papers),
                paper.title[:40],
                exc,
            )
            # Record failure as zero-score assessment with error
            assessments.append(
                Assessment(
                    paper_id=paper.id,
                    question_id=question.id,
                    score=0.0,
                    reasoning=f"Scoring failed: {exc}",
                    model=config.model,
                    temperature=config.temperature,
                    assessor=f"{config.model}:error",
                )
            )

    return assessments


def _parse_scoring_response(text: str) -> dict:
    """Parse Claude's JSON response into score and reasoning."""
    text = text.strip()

    # Handle markdown code blocks
    if text.startswith("```"):
        lines = text.split("\n")
        text = "\n".join(lines[1:-1]) if len(lines) > 2 else text

    try:
        data = json.loads(text)
        score = float(data.get("score", 0.0))
        score = max(0.0, min(1.0, score))  # clamp
        reasoning = str(data.get("reasoning", ""))
        return {"score": score, "reasoning": reasoning}
    except (json.JSONDecodeError, ValueError, TypeError):
        logger.warning("Failed to parse scoring response: %s", text[:200])
        return {"score": 0.0, "reasoning": f"Parse error. Raw response: {text[:500]}"}
