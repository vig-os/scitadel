"""CLI entry point for scitadel.

Thin wrapper over application services — business logic lives in services/.
"""

from __future__ import annotations

import click

from scitadel import __version__


@click.group()
@click.version_option(version=__version__, prog_name="scitadel")
def cli() -> None:
    """Scitadel — Programmable, reproducible scientific literature retrieval."""


@cli.command()
@click.argument("query")
@click.option(
    "--sources",
    "-s",
    default="pubmed,arxiv,openalex,inspire",
    help="Comma-separated list of sources to search.",
)
@click.option(
    "--max-results",
    "-n",
    default=50,
    type=int,
    help="Maximum results per source.",
)
def search(query: str, sources: str, max_results: int) -> None:
    """Run a federated literature search."""
    source_list = [s.strip() for s in sources.split(",")]
    click.echo(f"Searching {', '.join(source_list)} for: {query}")
    click.echo("(not yet implemented — see issue #10)")


@cli.command()
@click.option("--limit", "-n", default=20, type=int, help="Number of recent searches.")
def history(limit: int) -> None:
    """Show past search runs and parameters."""
    click.echo("(not yet implemented — see issue #13)")


@cli.command()
@click.argument("search_id")
@click.option(
    "--format",
    "-f",
    "fmt",
    type=click.Choice(["bibtex", "json", "csv"]),
    default="json",
    help="Export format.",
)
@click.option("--output", "-o", type=click.Path(), help="Output file path.")
def export(search_id: str, fmt: str, output: str | None) -> None:
    """Export search results in structured formats."""
    click.echo(f"Exporting search {search_id} as {fmt}")
    click.echo("(not yet implemented — see issue #12)")
