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
        assert "init" in result.output

    def test_init_command(self, tmp_path):
        runner = CliRunner()
        db_path = tmp_path / "test.db"
        result = runner.invoke(cli, ["init", "--db", str(db_path)])
        assert result.exit_code == 0
        assert "initialized" in result.output
        assert db_path.exists()

    def test_history_empty(self, tmp_path):
        runner = CliRunner()
        db_path = tmp_path / "test.db"
        # Init first
        runner.invoke(cli, ["init", "--db", str(db_path)])
        result = runner.invoke(cli, ["history"], env={"SCITADEL_DB": str(db_path)})
        assert result.exit_code == 0
        assert "No search history" in result.output

    def test_export_not_found(self, tmp_path):
        runner = CliRunner()
        db_path = tmp_path / "test.db"
        runner.invoke(cli, ["init", "--db", str(db_path)])
        result = runner.invoke(
            cli,
            ["export", "nonexistent"],
            env={"SCITADEL_DB": str(db_path)},
        )
        assert result.exit_code == 1
        assert "not found" in result.output
