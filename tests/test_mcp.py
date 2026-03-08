"""Tests for MCP server tool handlers.

Tests the tool functions directly (not via MCP transport)
to verify they correctly wrap the library API.
"""

from __future__ import annotations

import json

import pytest

from scitadel.domain.models import Paper, ResearchQuestion, Search, SearchResult
from scitadel.repositories.sqlite import (
    Database,
    SQLitePaperRepository,
    SQLiteResearchQuestionRepository,
    SQLiteSearchRepository,
)


@pytest.fixture()
def populated_db(tmp_path, monkeypatch):
    """Create a populated DB and point config at it."""
    db_path = tmp_path / "test.db"
    monkeypatch.setenv("SCITADEL_DB", str(db_path))

    db = Database(db_path)
    db.migrate()

    paper_repo = SQLitePaperRepository(db)
    search_repo = SQLiteSearchRepository(db)
    q_repo = SQLiteResearchQuestionRepository(db)

    # Papers
    papers = [
        Paper(
            id="paper1",
            title="PET Tracer for Cancer Detection",
            authors=["Smith, J", "Doe, A"],
            abstract="A study on FDG-PET imaging for oncology applications.",
            doi="10.1234/pet-cancer",
            year=2024,
            journal="Nature Medicine",
        ),
        Paper(
            id="paper2",
            title="Machine Learning in Drug Discovery",
            authors=["Alice, B"],
            abstract="ML methods for drug candidate screening.",
            year=2023,
        ),
    ]
    paper_repo.save_many(papers)

    # Search
    search = Search(
        id="search1",
        query="PET tracer oncology",
        sources=["pubmed", "arxiv"],
        total_papers=2,
    )
    search_repo.save(search)
    search_repo.save_results(
        [
            SearchResult(search_id="search1", paper_id="paper1", source="pubmed"),
            SearchResult(search_id="search1", paper_id="paper2", source="arxiv"),
        ]
    )

    # Research question
    question = ResearchQuestion(
        id="q1",
        text="What PET tracers are used in oncology?",
        description="Focus on clinical applications post-2020.",
    )
    q_repo.save_question(question)

    db.close()
    return db_path


class TestMCPTools:
    """Test MCP tool functions directly."""

    def test_list_searches(self, populated_db):
        from scitadel.mcp_server import list_searches

        result = list_searches()
        assert "search1" in result or "PET tracer" in result

    def test_get_papers(self, populated_db):
        from scitadel.mcp_server import get_papers

        result = get_papers("search1")
        assert "PET Tracer for Cancer Detection" in result
        assert "Machine Learning" in result
        assert "2 papers" in result

    def test_get_papers_prefix(self, populated_db):
        from scitadel.mcp_server import get_papers

        result = get_papers("search")
        assert "PET Tracer" in result

    def test_get_paper(self, populated_db):
        from scitadel.mcp_server import get_paper

        result = get_paper("paper1")
        data = json.loads(result)
        assert data["title"] == "PET Tracer for Cancer Detection"
        assert data["doi"] == "10.1234/pet-cancer"

    def test_export_search_json(self, populated_db):
        from scitadel.mcp_server import export_search

        result = export_search("search1", format="json")
        data = json.loads(result)
        assert len(data) == 2

    def test_export_search_bibtex(self, populated_db):
        from scitadel.mcp_server import export_search

        result = export_search("search1", format="bibtex")
        assert "@article{" in result

    def test_create_question(self, populated_db):
        from scitadel.mcp_server import create_question

        result = create_question("What is the role of AI in drug discovery?")
        assert "Question created" in result

    def test_list_questions(self, populated_db):
        from scitadel.mcp_server import list_questions

        result = list_questions()
        assert "PET tracers" in result

    def test_add_search_terms(self, populated_db):
        from scitadel.mcp_server import add_search_terms

        result = add_search_terms("q1", ["PET", "tracer", "oncology"])
        assert "Search terms added" in result

    def test_assess_paper(self, populated_db):
        from scitadel.mcp_server import assess_paper

        result = assess_paper(
            paper_id="paper1",
            question_id="q1",
            score=0.9,
            reasoning="Directly about PET in oncology.",
        )
        assert "Assessment saved" in result
        assert "0.90" in result

    def test_get_assessments(self, populated_db):
        from scitadel.mcp_server import assess_paper, get_assessments

        assess_paper(
            paper_id="paper1",
            question_id="q1",
            score=0.85,
            reasoning="Highly relevant.",
        )
        result = get_assessments(paper_id="paper1")
        assert "0.85" in result
        assert "Highly relevant" in result

    def test_get_papers_not_found(self, populated_db):
        from scitadel.mcp_server import get_papers

        result = get_papers("nonexistent")
        assert "not found" in result

    def test_assess_paper_not_found(self, populated_db):
        from scitadel.mcp_server import assess_paper

        result = assess_paper(
            paper_id="nonexistent",
            question_id="q1",
            score=0.5,
            reasoning="test",
        )
        assert "not found" in result


class TestCLIQuestionCommands:
    """Test the question CLI commands."""

    def test_question_create_and_list(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        env = {"SCITADEL_DB": str(db_path)}

        runner.invoke(cli, ["init", "--db", str(db_path)])

        result = runner.invoke(
            cli,
            ["question", "create", "What PET tracers are used?"],
            env=env,
        )
        assert result.exit_code == 0
        assert "Question ID" in result.output

        result = runner.invoke(cli, ["question", "list"], env=env)
        assert result.exit_code == 0
        assert "PET tracers" in result.output

    def test_question_list_empty(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])

        result = runner.invoke(
            cli, ["question", "list"], env={"SCITADEL_DB": str(db_path)}
        )
        assert result.exit_code == 0
        assert "No research questions" in result.output
