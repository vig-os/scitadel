"""Tests for Textual TUI application."""

from __future__ import annotations

from scitadel.domain.models import (
    Paper,
    Search,
    SearchResult,
    SourceOutcome,
    SourceStatus,
)


class TestTUIImports:
    """Verify TUI modules can be imported."""

    def test_import_main(self):
        from scitadel.tui import main

        assert callable(main)

    def test_import_app(self):
        from scitadel.tui.app import ScitadelApp

        assert ScitadelApp is not None

    def test_import_data_store(self):
        from scitadel.tui.data import DataStore

        assert DataStore is not None

    def test_import_screens(self):
        from scitadel.tui.screens.citation_tree import CitationTree
        from scitadel.tui.screens.live_search import LiveSearch
        from scitadel.tui.screens.paper_browser import PaperBrowser
        from scitadel.tui.screens.paper_detail import PaperDetail
        from scitadel.tui.screens.questions import QuestionsPanel
        from scitadel.tui.screens.search_browser import SearchBrowser

        assert all(
            [
                SearchBrowser,
                PaperBrowser,
                PaperDetail,
                QuestionsPanel,
                LiveSearch,
                CitationTree,
            ]
        )


class TestDataStore:
    """Test DataStore with in-memory DB."""

    def test_datastore_lifecycle(self, tmp_path, monkeypatch):
        monkeypatch.setenv("SCITADEL_DB", str(tmp_path / "test.db"))
        from scitadel.tui.data import DataStore

        store = DataStore()
        assert store.list_searches() == []
        assert store.list_questions() == []
        store.close()

    def test_datastore_papers(self, tmp_path, monkeypatch):
        monkeypatch.setenv("SCITADEL_DB", str(tmp_path / "test.db"))
        from scitadel.tui.data import DataStore

        store = DataStore()

        # Add a paper directly via internal repo
        paper = Paper(id="tp1", title="Test Paper", authors=["Alice"])
        store._papers.save(paper)

        result = store.get_paper("tp1")
        assert result is not None
        assert result.title == "Test Paper"

        papers = store.list_papers()
        assert len(papers) == 1
        store.close()

    def test_datastore_search_papers(self, tmp_path, monkeypatch):
        monkeypatch.setenv("SCITADEL_DB", str(tmp_path / "test.db"))
        from scitadel.tui.data import DataStore

        store = DataStore()

        paper = Paper(id="sp1", title="Search Paper")
        store._papers.save(paper)

        search = Search(
            id="s1",
            query="test",
            sources=["pubmed"],
            source_outcomes=[
                SourceOutcome(
                    source="pubmed",
                    status=SourceStatus.SUCCESS,
                    result_count=1,
                )
            ],
            total_papers=1,
        )
        store._searches.save(search)
        store._searches.save_results(
            [SearchResult(search_id="s1", paper_id="sp1", source="pubmed")]
        )

        papers = store.get_papers_for_search("s1")
        assert len(papers) == 1
        assert papers[0].id == "sp1"
        store.close()


class TestTUICommand:
    def test_tui_in_help(self):
        from click.testing import CliRunner

        from scitadel.cli import cli

        runner = CliRunner()
        result = runner.invoke(cli, ["--help"])
        assert "tui" in result.output
