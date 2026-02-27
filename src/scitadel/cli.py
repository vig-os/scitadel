"""CLI entry point for scitadel.

Thin wrapper over application services — business logic lives in services/.
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import click

from scitadel import __version__
from scitadel.config import load_config


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
    from scitadel.adapters.arxiv.adapter import ArxivAdapter
    from scitadel.adapters.inspire.adapter import InspireAdapter
    from scitadel.adapters.openalex.adapter import OpenAlexAdapter
    from scitadel.adapters.pubmed.adapter import PubMedAdapter
    from scitadel.repositories.sqlite import (
        Database,
        SQLitePaperRepository,
        SQLiteSearchRepository,
    )
    from scitadel.services.dedup import deduplicate
    from scitadel.services.orchestrator import run_search

    config = load_config()
    source_list = [s.strip() for s in sources.split(",")]

    adapter_map = {
        "pubmed": lambda: PubMedAdapter(api_key=config.pubmed.api_key),
        "arxiv": lambda: ArxivAdapter(),
        "openalex": lambda: OpenAlexAdapter(email=config.openalex.api_key),
        "inspire": lambda: InspireAdapter(),
    }

    adapters = []
    for name in source_list:
        if name in adapter_map:
            adapters.append(adapter_map[name]())
        else:
            click.echo(f"Unknown source: {name}", err=True)
            sys.exit(1)

    click.echo(f"Searching {', '.join(source_list)} for: {query}")

    search_record, candidates = asyncio.run(
        run_search(query, adapters, max_results=max_results)
    )

    click.echo(f"  Sources queried: {len(search_record.source_outcomes)}")
    for outcome in search_record.source_outcomes:
        status_icon = "+" if outcome.status.value == "success" else "!"
        click.echo(
            f"  [{status_icon}] {outcome.source}: "
            f"{outcome.result_count} results ({outcome.latency_ms:.0f}ms)"
            + (f" - {outcome.error}" if outcome.error else "")
        )
    click.echo(f"  Total candidates: {search_record.total_candidates}")

    papers, search_results = deduplicate(candidates)
    search_record = search_record.model_copy(update={"total_papers": len(papers)})
    click.echo(f"  Unique papers after dedup: {len(papers)}")

    # Persist
    db = Database(config.db_path)
    db.migrate()
    paper_repo = SQLitePaperRepository(db)
    search_repo = SQLiteSearchRepository(db)

    # Resolve new papers against existing DB records (match by DOI)
    # so we reuse existing IDs instead of creating duplicates.
    id_map: dict[str, str] = {}
    for paper in papers:
        if paper.doi:
            existing = paper_repo.find_by_doi(paper.doi)
            if existing and existing.id != paper.id:
                id_map[paper.id] = existing.id
                paper.id = existing.id

    paper_repo.save_many(papers)
    search_repo.save(search_record)

    for sr in search_results:
        sr.search_id = search_record.id
        sr.paper_id = id_map.get(sr.paper_id, sr.paper_id)
    search_repo.save_results(search_results)

    click.echo(f"\n  Search ID: {search_record.id}")
    click.echo(f"  Results saved to: {config.db_path}")
    db.close()


@cli.command()
@click.option("--limit", "-n", default=20, type=int, help="Number of recent searches.")
def history(limit: int) -> None:
    """Show past search runs and parameters."""
    from scitadel.repositories.sqlite import Database, SQLiteSearchRepository

    config = load_config()
    db = Database(config.db_path)
    db.migrate()
    search_repo = SQLiteSearchRepository(db)

    searches = search_repo.list_searches(limit=limit)
    if not searches:
        click.echo("No search history found.")
        db.close()
        return

    for s in searches:
        success_count = sum(1 for o in s.source_outcomes if o.status.value == "success")
        click.echo(
            f"  {s.id[:8]}  {s.created_at:%Y-%m-%d %H:%M}  "
            f'"{s.query}"  '
            f"{s.total_papers} papers  "
            f"{success_count}/{len(s.source_outcomes)} sources ok"
        )
    db.close()


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
    from scitadel.repositories.sqlite import (
        Database,
        SQLitePaperRepository,
        SQLiteSearchRepository,
    )
    from scitadel.services.export import export_bibtex, export_csv, export_json

    config = load_config()
    db = Database(config.db_path)
    db.migrate()
    search_repo = SQLiteSearchRepository(db)
    paper_repo = SQLitePaperRepository(db)

    # Support prefix matching on search IDs
    s = search_repo.get(search_id)
    if not s:
        searches = search_repo.list_searches(limit=100)
        matches = [sr for sr in searches if sr.id.startswith(search_id)]
        if len(matches) == 1:
            s = matches[0]
        elif len(matches) > 1:
            click.echo(f"Ambiguous search ID prefix '{search_id}'. Matches:")
            for m in matches:
                click.echo(f"  {m.id}")
            db.close()
            sys.exit(1)
        else:
            click.echo(f"Search '{search_id}' not found.")
            db.close()
            sys.exit(1)

    results = search_repo.get_results(s.id)
    paper_ids = {r.paper_id for r in results}
    papers = [p for pid in paper_ids if (p := paper_repo.get(pid)) is not None]

    formatters = {
        "json": export_json,
        "csv": export_csv,
        "bibtex": export_bibtex,
    }
    content = formatters[fmt](papers)

    if output:
        Path(output).write_text(content)
        click.echo(f"Exported {len(papers)} papers to {output}")
    else:
        click.echo(content)

    db.close()


@cli.command()
@click.option(
    "--db", type=click.Path(), help="Database path (default: ~/.scitadel/scitadel.db)"
)
def init(db: str | None) -> None:
    """Initialize the scitadel database."""
    from scitadel.repositories.sqlite import Database

    config = load_config()
    db_path = Path(db) if db else config.db_path
    database = Database(db_path)
    database.migrate()
    click.echo(f"Database initialized at: {db_path}")
    database.close()
