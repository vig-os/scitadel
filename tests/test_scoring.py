"""Tests for LLM scoring service."""

from __future__ import annotations

from unittest.mock import MagicMock

from scitadel.domain.models import Assessment, Paper, ResearchQuestion
from scitadel.services.scoring import (
    ScoringConfig,
    _parse_scoring_response,
    score_paper,
    score_papers,
)


class TestParseResponse:
    def test_valid_json(self):
        result = _parse_scoring_response(
            '{"score": 0.85, "reasoning": "Highly relevant."}'
        )
        assert result["score"] == 0.85
        assert result["reasoning"] == "Highly relevant."

    def test_json_in_code_block(self):
        result = _parse_scoring_response(
            '```json\n{"score": 0.5, "reasoning": "Moderate."}\n```'
        )
        assert result["score"] == 0.5

    def test_clamps_score(self):
        result = _parse_scoring_response('{"score": 1.5, "reasoning": "Over."}')
        assert result["score"] == 1.0

        result = _parse_scoring_response('{"score": -0.5, "reasoning": "Under."}')
        assert result["score"] == 0.0

    def test_invalid_json(self):
        result = _parse_scoring_response("This is not JSON")
        assert result["score"] == 0.0
        assert "Parse error" in result["reasoning"]

    def test_empty_response(self):
        result = _parse_scoring_response("")
        assert result["score"] == 0.0


def _mock_client(response_text: str) -> MagicMock:
    """Create a mock Anthropic client that returns a fixed response."""
    client = MagicMock()
    message = MagicMock()
    content_block = MagicMock()
    content_block.text = response_text
    message.content = [content_block]
    client.messages.create.return_value = message
    return client


class TestScorePaper:
    def test_score_single_paper(self):
        paper = Paper(
            id="p1",
            title="PET Tracer for Oncology",
            abstract="A study on FDG-PET imaging for cancer detection.",
            authors=["Smith, J"],
            year=2024,
        )
        question = ResearchQuestion(
            id="q1", text="What PET tracers are used in oncology?"
        )

        client = _mock_client(
            '{"score": 0.92, "reasoning": "Directly addresses PET in oncology."}'
        )
        config = ScoringConfig(model="claude-test", temperature=0.0)

        assessment = score_paper(paper, question, config=config, client=client)

        assert isinstance(assessment, Assessment)
        assert assessment.paper_id == "p1"
        assert assessment.question_id == "q1"
        assert assessment.score == 0.92
        assert assessment.reasoning == "Directly addresses PET in oncology."
        assert assessment.model == "claude-test"
        assert assessment.assessor == "claude-test"
        assert assessment.temperature == 0.0
        assert assessment.prompt  # should contain the paper abstract

        # Verify the API was called with correct params
        client.messages.create.assert_called_once()
        call_kwargs = client.messages.create.call_args[1]
        assert call_kwargs["model"] == "claude-test"
        assert call_kwargs["temperature"] == 0.0

    def test_score_paper_provenance(self):
        paper = Paper(id="p2", title="Some Paper", abstract="Abstract text.")
        question = ResearchQuestion(id="q2", text="Test question?")
        client = _mock_client('{"score": 0.5, "reasoning": "Moderate."}')

        assessment = score_paper(paper, question, client=client)

        # Check provenance fields
        assert assessment.prompt is not None
        assert "Test question?" in assessment.prompt
        assert "Some Paper" in assessment.prompt
        assert "Abstract text." in assessment.prompt


class TestScorePapers:
    def test_score_multiple_papers(self):
        papers = [
            Paper(id=f"p{i}", title=f"Paper {i}", abstract=f"Abstract {i}.")
            for i in range(3)
        ]
        question = ResearchQuestion(id="q1", text="Test?")
        client = _mock_client('{"score": 0.7, "reasoning": "Relevant."}')

        assessments = score_papers(papers, question, client=client)

        assert len(assessments) == 3
        assert all(a.score == 0.7 for a in assessments)
        assert client.messages.create.call_count == 3

    def test_handles_api_failure(self):
        papers = [Paper(id="p1", title="Paper 1", abstract="Abstract.")]
        question = ResearchQuestion(id="q1", text="Test?")

        client = MagicMock()
        client.messages.create.side_effect = Exception("API error")

        assessments = score_papers(papers, question, client=client)

        assert len(assessments) == 1
        assert assessments[0].score == 0.0
        assert "Scoring failed" in assessments[0].reasoning
        assert "error" in assessments[0].assessor

    def test_progress_callback(self):
        papers = [Paper(id="p1", title="Paper 1", abstract="Abstract.")]
        question = ResearchQuestion(id="q1", text="Test?")
        client = _mock_client('{"score": 0.8, "reasoning": "Good."}')

        progress_calls = []
        score_papers(
            papers,
            question,
            client=client,
            on_progress=lambda i, total, paper, a: progress_calls.append((i, total)),
        )

        assert len(progress_calls) == 1
        assert progress_calls[0] == (0, 1)
