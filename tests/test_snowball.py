"""Tests for snowball (citation chaining) service and related components."""

from __future__ import annotations

import asyncio

import pytest

from scitadel.domain.models import (
    Citation,
    CitationDirection,
    Paper,
    ResearchQuestion,
    SnowballRun,
)
from scitadel.repositories.sqlite import (
    Database,
    SQLiteCitationRepository,
    SQLitePaperRepository,
    SQLiteResearchQuestionRepository,
)
from scitadel.services.snowball import SnowballConfig, snowball


# -- Domain model tests --


class TestCitationModels:
    def test_citation_direction_enum(self):
        assert CitationDirection.REFERENCES.value == "references"
        assert CitationDirection.CITED_BY.value == "cited_by"

    def test_citation_create(self):
        c = Citation(
            source_paper_id="p1",
            target_paper_id="p2",
            direction=CitationDirection.REFERENCES,
            discovered_by="openalex",
            depth=1,
        )
        assert c.source_paper_id == "p1"
        assert c.target_paper_id == "p2"
        assert c.direction == CitationDirection.REFERENCES

    def test_snowball_run_create(self):
        run = SnowballRun(
            question_id="q1",
            direction="both",
            max_depth=2,
            threshold=0.6,
            total_discovered=10,
            total_new_papers=5,
        )
        assert run.question_id == "q1"
        assert run.max_depth == 2
        assert run.total_new_papers == 5


# -- Repository tests --


@pytest.fixture()
def db():
    database = Database(":memory:")
    database.migrate()
    yield database
    database.close()


@pytest.fixture()
def paper_repo(db):
    return SQLitePaperRepository(db)


@pytest.fixture()
def citation_repo(db):
    return SQLiteCitationRepository(db)


@pytest.fixture()
def question_repo(db):
    return SQLiteResearchQuestionRepository(db)


class TestCitationRepository:
    def test_save_and_get_references(self, citation_repo, paper_repo):
        paper_repo.save(Paper(id="p1", title="Paper 1"))
        paper_repo.save(Paper(id="p2", title="Paper 2"))

        c = Citation(
            source_paper_id="p1",
            target_paper_id="p2",
            direction=CitationDirection.REFERENCES,
            discovered_by="openalex",
            depth=1,
        )
        citation_repo.save(c)

        refs = citation_repo.get_references("p1")
        assert len(refs) == 1
        assert refs[0].target_paper_id == "p2"

    def test_save_many(self, citation_repo, paper_repo):
        for i in range(4):
            paper_repo.save(Paper(id=f"p{i}", title=f"Paper {i}"))

        citations = [
            Citation(
                source_paper_id="p0",
                target_paper_id=f"p{i}",
                direction=CitationDirection.REFERENCES,
                depth=1,
            )
            for i in range(1, 4)
        ]
        citation_repo.save_many(citations)

        refs = citation_repo.get_references("p0")
        assert len(refs) == 3

    def test_upsert_keeps_minimum_depth(self, citation_repo, paper_repo):
        paper_repo.save(Paper(id="p1", title="Paper 1"))
        paper_repo.save(Paper(id="p2", title="Paper 2"))

        c1 = Citation(
            source_paper_id="p1",
            target_paper_id="p2",
            direction=CitationDirection.REFERENCES,
            depth=3,
        )
        citation_repo.save(c1)

        c2 = Citation(
            source_paper_id="p1",
            target_paper_id="p2",
            direction=CitationDirection.REFERENCES,
            depth=1,
        )
        citation_repo.save(c2)

        refs = citation_repo.get_references("p1")
        assert len(refs) == 1
        assert refs[0].depth == 1

    def test_exists(self, citation_repo, paper_repo):
        paper_repo.save(Paper(id="p1", title="P1"))
        paper_repo.save(Paper(id="p2", title="P2"))

        assert not citation_repo.exists("p1", "p2", "references")

        citation_repo.save(
            Citation(
                source_paper_id="p1",
                target_paper_id="p2",
                direction=CitationDirection.REFERENCES,
            )
        )
        assert citation_repo.exists("p1", "p2", "references")

    def test_get_citations_cited_by(self, citation_repo, paper_repo):
        paper_repo.save(Paper(id="pa", title="A"))
        paper_repo.save(Paper(id="pb", title="B"))

        citation_repo.save(
            Citation(
                source_paper_id="pa",
                target_paper_id="pb",
                direction=CitationDirection.CITED_BY,
            )
        )

        cites = citation_repo.get_citations("pb")
        assert len(cites) == 1
        assert cites[0].source_paper_id == "pa"

    def test_snowball_run_persistence(self, citation_repo, question_repo):
        question_repo.save_question(ResearchQuestion(id="q1", text="Test Q"))
        run = SnowballRun(
            id="sr1",
            question_id="q1",
            direction="both",
            max_depth=2,
            threshold=0.5,
            total_discovered=15,
            total_new_papers=8,
        )
        citation_repo.save_snowball_run(run)

        result = citation_repo.get_snowball_run("sr1")
        assert result is not None
        assert result.total_discovered == 15
        assert result.max_depth == 2

    def test_list_snowball_runs(self, citation_repo, question_repo):
        question_repo.save_question(ResearchQuestion(id="ql", text="Test"))
        for i in range(3):
            citation_repo.save_snowball_run(
                SnowballRun(id=f"sr{i}", direction="both", question_id="ql")
            )
        runs = citation_repo.list_snowball_runs()
        assert len(runs) == 3


# -- Snowball service tests --


class MockFetcher:
    """Mock citation fetcher for testing."""

    def __init__(self, references=None, cited_by=None):
        self._references = references or {}
        self._cited_by = cited_by or {}

    async def fetch_references(self, paper):
        return self._references.get(paper.id, [])

    async def fetch_cited_by(self, paper):
        return self._cited_by.get(paper.id, [])


class MockResolver:
    """Mock paper resolver for testing."""

    def __init__(self, papers=None):
        self._papers = {p.id: p for p in (papers or [])}
        self._title_map = {p.title.lower(): p for p in (papers or [])}

    def resolve(self, work_dict):
        title = work_dict.get("title", "")
        if title.lower() in self._title_map:
            return self._title_map[title.lower()], False
        paper = Paper(title=title, doi=work_dict.get("doi"))
        self._papers[paper.id] = paper
        self._title_map[title.lower()] = paper
        return paper, True


class MockScorer:
    """Mock scorer for testing."""

    def __init__(self, scores=None):
        self._scores = scores or {}
        self._default = 0.5

    def score(self, paper, question):
        return self._scores.get(paper.title, self._default)


class TestSnowballService:
    def test_basic_snowball(self):
        seed = Paper(id="seed1", title="Seed Paper")
        question = ResearchQuestion(id="q1", text="Test question")

        ref_work = {"title": "Referenced Paper", "doi": "10.1234/ref"}
        fetcher = MockFetcher(references={"seed1": [ref_work]})
        resolver = MockResolver()
        scorer = MockScorer(scores={"Referenced Paper": 0.8})

        config = SnowballConfig(direction="references", max_depth=1, threshold=0.6)

        run, citations, new_papers = asyncio.run(
            snowball(
                [seed],
                question,
                fetcher=fetcher,
                resolver=resolver,
                scorer=scorer,
                config=config,
            )
        )

        assert run.total_discovered == 1
        assert run.total_new_papers == 1
        assert len(citations) == 1
        assert citations[0].direction == CitationDirection.REFERENCES

    def test_threshold_gating(self):
        """Papers below threshold should not be added to frontier."""
        seed = Paper(id="seed1", title="Seed Paper")
        question = ResearchQuestion(id="q1", text="Test question")

        ref_work = {"title": "Low Score Paper"}

        fetcher = MockFetcher(
            references={
                "seed1": [ref_work],
            }
        )
        resolver = MockResolver()
        scorer = MockScorer(
            scores={"Low Score Paper": 0.3, "Should Not Reach": 0.9}
        )

        config = SnowballConfig(direction="references", max_depth=2, threshold=0.6)

        run, citations, new_papers = asyncio.run(
            snowball(
                [seed],
                question,
                fetcher=fetcher,
                resolver=resolver,
                scorer=scorer,
                config=config,
            )
        )

        # Low-scoring paper is discovered but doesn't expand
        assert run.total_discovered == 1
        titles = {p.title for p in new_papers}
        assert "Should Not Reach" not in titles

    def test_depth_limiting(self):
        """Snowball should respect max_depth."""
        seed = Paper(id="seed1", title="Seed")
        question = ResearchQuestion(id="q1", text="Q")

        fetcher = MockFetcher(
            references={
                "seed1": [{"title": "Level 1"}],
            }
        )
        resolver = MockResolver()
        scorer = MockScorer(scores={"Level 1": 0.9})

        config = SnowballConfig(direction="references", max_depth=1, threshold=0.5)

        run, citations, new_papers = asyncio.run(
            snowball(
                [seed],
                question,
                fetcher=fetcher,
                resolver=resolver,
                scorer=scorer,
                config=config,
            )
        )

        assert run.max_depth == 1
        assert run.total_discovered == 1

    def test_dedup_within_snowball(self):
        """Same paper from multiple seeds should only appear once."""
        seed1 = Paper(id="s1", title="Seed 1")
        seed2 = Paper(id="s2", title="Seed 2")
        question = ResearchQuestion(id="q1", text="Q")

        shared = {"title": "Shared Paper"}
        fetcher = MockFetcher(references={"s1": [shared], "s2": [shared]})
        resolver = MockResolver()
        scorer = MockScorer(scores={"Shared Paper": 0.8})

        config = SnowballConfig(direction="references", max_depth=1)

        run, citations, new_papers = asyncio.run(
            snowball(
                [seed1, seed2],
                question,
                fetcher=fetcher,
                resolver=resolver,
                scorer=scorer,
                config=config,
            )
        )

        assert run.total_discovered == 1
        assert run.total_new_papers == 1

    def test_both_directions(self):
        """Test snowballing in both directions."""
        seed = Paper(id="seed1", title="Seed")
        question = ResearchQuestion(id="q1", text="Q")

        fetcher = MockFetcher(
            references={"seed1": [{"title": "Ref Paper"}]},
            cited_by={"seed1": [{"title": "Citing Paper"}]},
        )
        resolver = MockResolver()
        scorer = MockScorer(
            scores={"Ref Paper": 0.8, "Citing Paper": 0.7}
        )

        config = SnowballConfig(direction="both", max_depth=1, threshold=0.5)

        run, citations, new_papers = asyncio.run(
            snowball(
                [seed],
                question,
                fetcher=fetcher,
                resolver=resolver,
                scorer=scorer,
                config=config,
            )
        )

        assert run.total_discovered == 2
        assert run.total_new_papers == 2
        directions = {c.direction for c in citations}
        assert CitationDirection.REFERENCES in directions
        assert CitationDirection.CITED_BY in directions

    def test_on_progress_callback(self):
        seed = Paper(id="seed1", title="Seed")
        question = ResearchQuestion(id="q1", text="Q")

        fetcher = MockFetcher(references={"seed1": [{"title": "Ref"}]})
        resolver = MockResolver()
        scorer = MockScorer(scores={"Ref": 0.7})

        progress_calls = []

        def on_progress(depth, paper, score, is_new):
            progress_calls.append((depth, paper.title, score, is_new))

        config = SnowballConfig(direction="references", max_depth=1)

        asyncio.run(
            snowball(
                [seed],
                question,
                fetcher=fetcher,
                resolver=resolver,
                scorer=scorer,
                config=config,
                on_progress=on_progress,
            )
        )

        assert len(progress_calls) == 1
        assert progress_calls[0][0] == 1  # depth
        assert progress_calls[0][1] == "Ref"


# -- CLI tests --


class TestSnowballCLI:
    def test_help(self):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        result = runner.invoke(cli, ["snowball", "--help"])
        assert result.exit_code == 0
        assert "citation chaining" in result.output.lower()

    def test_search_not_found(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])
        result = runner.invoke(
            cli,
            ["snowball", "nonexistent", "-q", "q1"],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 1
        assert "not found" in result.output


# -- Question add-terms CLI tests --


class TestQuestionAddTermsCLI:
    def test_add_terms(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])

        # Create a question first
        result = runner.invoke(
            cli,
            ["question", "create", "What PET tracers exist?"],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 0
        # Extract question ID
        for line in result.output.split("\n"):
            if "Question ID:" in line:
                q_id = line.split(":")[1].strip()
                break

        # Add terms
        result = runner.invoke(
            cli,
            ["question", "add-terms", q_id[:8], "PET", "tracer", "oncology"],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 0
        assert "Terms added" in result.output

    def test_add_terms_with_query(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])

        result = runner.invoke(
            cli,
            ["question", "create", "Test question"],
            env={"SCITADEL_DB": str(db_path)},
        )
        for line in result.output.split("\n"):
            if "Question ID:" in line:
                q_id = line.split(":")[1].strip()
                break

        result = runner.invoke(
            cli,
            [
                "question",
                "add-terms",
                q_id[:8],
                "PET",
                "tracer",
                "--query",
                "PET AND tracer",
            ],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 0
        assert "PET AND tracer" in result.output

    def test_add_terms_question_not_found(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])

        result = runner.invoke(
            cli,
            ["question", "add-terms", "nonexistent", "term1"],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 1
        assert "not found" in result.output


# -- Search --question CLI tests --


class TestSearchQuestionCLI:
    def test_search_no_query_no_question(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])

        result = runner.invoke(
            cli,
            ["search"],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 1
        assert "Provide a QUERY" in result.output

    def test_search_question_not_found(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])

        result = runner.invoke(
            cli,
            ["search", "-q", "nonexistent"],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 1
        assert "not found" in result.output

    def test_search_question_no_terms(self, tmp_path):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])

        # Create question without terms
        result = runner.invoke(
            cli,
            ["question", "create", "Test question"],
            env={"SCITADEL_DB": str(db_path)},
        )
        for line in result.output.split("\n"):
            if "Question ID:" in line:
                q_id = line.split(":")[1].strip()
                break

        result = runner.invoke(
            cli,
            ["search", "-q", q_id[:8]],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 1
        assert "No search terms" in result.output
