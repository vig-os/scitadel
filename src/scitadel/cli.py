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
@click.argument("query", required=False, default=None)
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
@click.option(
    "--question",
    "-q",
    "question_id",
    default=None,
    help="Research question ID — auto-builds query from linked search terms.",
)
def search(
    query: str | None, sources: str, max_results: int, question_id: str | None
) -> None:
    """Run a federated literature search.

    QUERY can be omitted when --question is provided (uses linked search terms).
    """
    from scitadel.adapters import build_adapters
    from scitadel.repositories.sqlite import (
        Database,
        SQLitePaperRepository,
        SQLiteResearchQuestionRepository,
        SQLiteSearchRepository,
    )
    from scitadel.services.dedup import deduplicate
    from scitadel.services.orchestrator import run_search

    config = load_config()
    db = Database(config.db_path)
    db.migrate()

    parameters: dict = {}

    # Resolve question-driven query
    if question_id:
        q_repo = SQLiteResearchQuestionRepository(db)
        q = q_repo.get_question(question_id)
        if not q:
            questions = q_repo.list_questions()
            matches = [rq for rq in questions if rq.id.startswith(question_id)]
            if len(matches) == 1:
                q = matches[0]
            else:
                click.echo(f"Question '{question_id}' not found.", err=True)
                db.close()
                sys.exit(1)

        parameters["question_id"] = q.id

        if not query:
            terms = q_repo.get_terms(q.id)
            if not terms:
                click.echo(
                    f"No search terms linked to question '{q.id[:8]}'. "
                    "Add terms with: scitadel question add-terms",
                    err=True,
                )
                db.close()
                sys.exit(1)
            query = " OR ".join(t.query_string for t in terms if t.query_string)
            if not query:
                click.echo("Linked search terms have no query strings.", err=True)
                db.close()
                sys.exit(1)
            click.echo(f"  Auto-built query from {len(terms)} term group(s)")

    if not query:
        click.echo("Provide a QUERY argument or use --question.", err=True)
        db.close()
        sys.exit(1)

    source_list = [s.strip() for s in sources.split(",")]
    try:
        adapters = build_adapters(
            source_list,
            pubmed_api_key=config.pubmed.api_key,
            openalex_email=config.openalex.api_key,
        )
    except ValueError as e:
        click.echo(str(e), err=True)
        db.close()
        sys.exit(1)

    click.echo(f"Searching {', '.join(source_list)} for: {query}")

    search_record, candidates = asyncio.run(
        run_search(query, adapters, max_results=max_results)
    )
    search_record = search_record.model_copy(
        update={"parameters": {**search_record.parameters, **parameters}}
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


# -- Research question commands --


@cli.group()
def question() -> None:
    """Manage research questions."""


@question.command("create")
@click.argument("text")
@click.option("--description", "-d", default="", help="Additional context.")
def question_create(text: str, description: str) -> None:
    """Create a research question for relevance scoring."""
    from scitadel.domain.models import ResearchQuestion
    from scitadel.repositories.sqlite import Database, SQLiteResearchQuestionRepository

    config = load_config()
    db = Database(config.db_path)
    db.migrate()
    q_repo = SQLiteResearchQuestionRepository(db)

    q = ResearchQuestion(text=text, description=description)
    q_repo.save_question(q)
    click.echo(f"  Question ID: {q.id}")
    click.echo(f"  Text: {text}")
    db.close()


@question.command("list")
def question_list() -> None:
    """List all research questions."""
    from scitadel.repositories.sqlite import Database, SQLiteResearchQuestionRepository

    config = load_config()
    db = Database(config.db_path)
    db.migrate()
    q_repo = SQLiteResearchQuestionRepository(db)

    questions = q_repo.list_questions()
    if not questions:
        click.echo("No research questions found.")
        db.close()
        return

    for q in questions:
        click.echo(f'  {q.id[:8]}  {q.created_at:%Y-%m-%d %H:%M}  "{q.text}"')
    db.close()


@question.command("add-terms")
@click.argument("question_id")
@click.argument("terms", nargs=-1, required=True)
@click.option("--query", "-q", "query_string", default="", help="Custom query string.")
def question_add_terms(question_id: str, terms: tuple[str, ...], query_string: str) -> None:
    """Add search terms linked to a research question."""
    from scitadel.domain.models import SearchTerm
    from scitadel.repositories.sqlite import Database, SQLiteResearchQuestionRepository

    config = load_config()
    db = Database(config.db_path)
    db.migrate()
    q_repo = SQLiteResearchQuestionRepository(db)

    # Resolve prefix
    q = q_repo.get_question(question_id)
    if not q:
        questions = q_repo.list_questions()
        matches = [rq for rq in questions if rq.id.startswith(question_id)]
        if len(matches) == 1:
            q = matches[0]
        else:
            click.echo(f"Question '{question_id}' not found.")
            db.close()
            sys.exit(1)

    if not query_string:
        query_string = " ".join(terms)

    term = SearchTerm(
        question_id=q.id,
        terms=list(terms),
        query_string=query_string,
    )
    q_repo.save_term(term)
    click.echo(f"  Terms added to question {q.id[:8]}: {list(terms)}")
    click.echo(f"  Query string: {query_string}")
    db.close()


# -- Assess command --


@cli.command()
@click.argument("search_id")
@click.option(
    "--question", "-q", "question_id", required=True, help="Research question ID."
)
@click.option(
    "--model",
    "-m",
    default="claude-sonnet-4-6",
    help="Model for scoring (default: claude-sonnet-4-6).",
)
@click.option(
    "--temperature",
    "-t",
    default=0.0,
    type=float,
    help="Temperature for scoring (default: 0.0).",
)
def assess(search_id: str, question_id: str, model: str, temperature: float) -> None:
    """Score papers from a search against a research question using Claude.

    Requires ANTHROPIC_API_KEY environment variable.
    """
    from scitadel.repositories.sqlite import (
        Database,
        SQLiteAssessmentRepository,
        SQLitePaperRepository,
        SQLiteResearchQuestionRepository,
        SQLiteSearchRepository,
    )
    from scitadel.services.scoring import ScoringConfig, score_papers

    config = load_config()
    db = Database(config.db_path)
    db.migrate()
    search_repo = SQLiteSearchRepository(db)
    paper_repo = SQLitePaperRepository(db)
    q_repo = SQLiteResearchQuestionRepository(db)
    a_repo = SQLiteAssessmentRepository(db)

    # Resolve search ID
    s = search_repo.get(search_id)
    if not s:
        searches = search_repo.list_searches(limit=100)
        matches = [sr for sr in searches if sr.id.startswith(search_id)]
        if len(matches) == 1:
            s = matches[0]
        else:
            click.echo(f"Search '{search_id}' not found.")
            db.close()
            sys.exit(1)

    # Resolve question ID
    q = q_repo.get_question(question_id)
    if not q:
        questions = q_repo.list_questions()
        matches = [rq for rq in questions if rq.id.startswith(question_id)]
        if len(matches) == 1:
            q = matches[0]
        else:
            click.echo(f"Question '{question_id}' not found.")
            db.close()
            sys.exit(1)

    # Load papers
    results = search_repo.get_results(s.id)
    paper_ids = {r.paper_id for r in results}
    papers = [p for pid in paper_ids if (p := paper_repo.get(pid))]

    click.echo(f'Scoring {len(papers)} papers against: "{q.text}"')
    click.echo(f"  Model: {model}  Temperature: {temperature}")

    scoring_config = ScoringConfig(
        model=model,
        temperature=temperature,
    )

    def on_progress(i, total, paper, assessment):
        click.echo(f"  [{i + 1}/{total}] {assessment.score:.2f}  {paper.title[:60]}")

    assessments = score_papers(
        papers, q, config=scoring_config, on_progress=on_progress
    )

    # Persist
    for a in assessments:
        a_repo.save(a)

    # Summary
    scores = [a.score for a in assessments]
    avg = sum(scores) / len(scores) if scores else 0
    relevant = sum(1 for s in scores if s >= 0.6)
    click.echo(f"\n  Scored: {len(assessments)} papers")
    click.echo(f"  Average relevance: {avg:.2f}")
    click.echo(f"  Relevant (≥0.6): {relevant}/{len(assessments)}")
    db.close()


# -- Snowball command --


@cli.command()
@click.argument("search_id")
@click.option(
    "--question", "-q", "question_id", required=True, help="Research question ID."
)
@click.option("--depth", "-d", default=1, type=int, help="Max chaining depth (1-3).")
@click.option(
    "--threshold", default=0.6, type=float, help="Min relevance score to expand."
)
@click.option(
    "--direction",
    type=click.Choice(["references", "cited_by", "both"]),
    default="both",
    help="Citation direction.",
)
@click.option(
    "--model",
    "-m",
    default="claude-sonnet-4-6",
    help="Model for scoring.",
)
def snowball(
    search_id: str,
    question_id: str,
    depth: int,
    threshold: float,
    direction: str,
    model: str,
) -> None:
    """Run citation chaining (snowballing) from a search's papers."""
    from scitadel.adapters.openalex.citations import (
        OpenAlexCitationFetcher,
        work_to_paper_dict,
    )
    from scitadel.domain.models import Paper
    from scitadel.repositories.sqlite import (
        Database,
        SQLiteCitationRepository,
        SQLitePaperRepository,
        SQLiteResearchQuestionRepository,
        SQLiteSearchRepository,
    )
    from scitadel.services.scoring import ScoringConfig, score_paper
    from scitadel.services.snowball import SnowballConfig
    from scitadel.services.snowball import snowball as run_snowball

    config = load_config()
    db = Database(config.db_path)
    db.migrate()
    search_repo = SQLiteSearchRepository(db)
    paper_repo = SQLitePaperRepository(db)
    q_repo = SQLiteResearchQuestionRepository(db)
    citation_repo = SQLiteCitationRepository(db)

    # Resolve search ID
    s = search_repo.get(search_id)
    if not s:
        searches = search_repo.list_searches(limit=100)
        matches = [sr for sr in searches if sr.id.startswith(search_id)]
        if len(matches) == 1:
            s = matches[0]
        else:
            click.echo(f"Search '{search_id}' not found.")
            db.close()
            sys.exit(1)

    # Resolve question ID
    q = q_repo.get_question(question_id)
    if not q:
        questions = q_repo.list_questions()
        matches = [rq for rq in questions if rq.id.startswith(question_id)]
        if len(matches) == 1:
            q = matches[0]
        else:
            click.echo(f"Question '{question_id}' not found.")
            db.close()
            sys.exit(1)

    # Load seed papers
    results = search_repo.get_results(s.id)
    paper_ids = {r.paper_id for r in results}
    seed_papers = [p for pid in paper_ids if (p := paper_repo.get(pid))]

    click.echo(
        f"Snowballing from {len(seed_papers)} papers "
        f"(depth={depth}, direction={direction}, threshold={threshold})"
    )

    fetcher = OpenAlexCitationFetcher(email=config.openalex.api_key)
    scoring_config = ScoringConfig(model=model)

    import anthropic

    client = anthropic.Anthropic()

    class _Resolver:
        def resolve(self, work_dict: dict) -> tuple[Paper, bool]:
            kwargs = work_to_paper_dict(work_dict)
            # Check for existing paper by DOI
            if kwargs.get("doi"):
                existing = paper_repo.find_by_doi(kwargs["doi"])
                if existing:
                    return existing, False
            # Check by title
            if kwargs.get("title"):
                existing = paper_repo.find_by_title(kwargs["title"])
                if existing:
                    return existing, False
            paper = Paper(**kwargs)
            paper_repo.save(paper)
            return paper, True

    class _Scorer:
        def score(self, paper: Paper, question):
            assessment = score_paper(
                paper, question, config=scoring_config, client=client
            )
            return assessment.score

    snowball_config = SnowballConfig(
        direction=direction,
        max_depth=depth,
        threshold=threshold,
    )

    def on_progress(d, paper, score, is_new):
        marker = "NEW" if is_new else "   "
        click.echo(
            f"  [d{d}] {score:.2f} {marker}  {paper.title[:55]}"
        )

    run, citations, new_papers = asyncio.run(
        run_snowball(
            seed_papers,
            q,
            fetcher=fetcher,
            resolver=_Resolver(),
            scorer=_Scorer(),
            config=snowball_config,
            on_progress=on_progress,
        )
    )

    run = run.model_copy(update={"search_id": s.id})

    # Persist
    citation_repo.save_many(citations)
    citation_repo.save_snowball_run(run)

    click.echo(f"\n  Snowball run: {run.id[:8]}")
    click.echo(f"  Discovered: {run.total_discovered} papers")
    click.echo(f"  New papers: {run.total_new_papers}")
    click.echo(f"  Citation edges: {len(citations)}")
    db.close()


# -- TUI command --


@cli.command()
def tui() -> None:
    """Launch the interactive TUI dashboard."""
    try:
        from scitadel.tui import main as tui_main
    except ImportError:
        click.echo(
            "Textual is not installed. Install with: pip install scitadel[tui]",
            err=True,
        )
        sys.exit(1)
    tui_main()
