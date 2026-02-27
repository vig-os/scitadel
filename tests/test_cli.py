"""Tests for CLI entry point."""

from click.testing import CliRunner

from scitadel.cli import cli


class TestCLI:
    def test_version(self):
        runner = CliRunner()
        result = runner.invoke(cli, ["--version"])
        assert result.exit_code == 0
        assert "0.1.0" in result.output

    def test_help(self):
        runner = CliRunner()
        result = runner.invoke(cli, ["--help"])
        assert result.exit_code == 0
        assert "search" in result.output
        assert "history" in result.output
        assert "export" in result.output

    def test_search_placeholder(self):
        runner = CliRunner()
        result = runner.invoke(cli, ["search", "PET tracer"])
        assert result.exit_code == 0
        assert "Searching" in result.output

    def test_history_placeholder(self):
        runner = CliRunner()
        result = runner.invoke(cli, ["history"])
        assert result.exit_code == 0

    def test_export_placeholder(self):
        runner = CliRunner()
        result = runner.invoke(cli, ["export", "test-id", "-f", "bibtex"])
        assert result.exit_code == 0
