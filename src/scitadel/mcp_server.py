"""Scitadel MCP server — exposes literature search tools to LLM agents.

Run with: scitadel-mcp
Or configure in Claude Desktop / Cursor / Claude CLI as an MCP server.
"""

from __future__ import annotations

import json

from mcp.server.fastmcp import FastMCP

from scitadel.config import load_config
from scitadel.repositories.sqlite import (
    Database,
    SQLiteAssessmentRepository,
    SQLitePaperRepository,
    SQLiteResearchQuestionRepository,
    SQLiteSearchRepository,
)
from scitadel.services.dedup import deduplicate
from scitadel.services.export import export_bibtex, export_csv, export_json

mcp = FastMCP(
    "scitadel",
    instructions="Programmable, reproducible scientific literature retrieval. "
    "Use these tools to search scientific databases, manage research questions, "
    "and score paper relevance.",
)


def _get_db() -> Database:
    config = load_config()
    db = Database(config.db_path)
    db.migrate()
    return db


# -- Search tools --


@mcp.tool()
async def search(
    query: str,
    sources: str = "pubmed,arxiv,openalex,inspire",
    max_results: int = 50,
) -> str:
    """Run a federated literature search across scientific databases.

    Searches PubMed, arXiv, OpenAlex, and INSPIRE-HEP in parallel.
    Results are deduplicated and persisted to the local database.
    Returns the search ID and summary statistics.
    """
    from scitadel.adapters.arxiv.adapter import ArxivAdapter
    from scitadel.adapters.inspire.adapter import InspireAdapter
    from scitadel.adapters.openalex.adapter import OpenAlexAdapter
    from scitadel.adapters.pubmed.adapter import PubMedAdapter
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

    search_record, candidates = await run_search(
        query, adapters, max_results=max_results
    )

    papers, search_results = deduplicate(candidates)
    search_record = search_record.model_copy(update={"total_papers": len(papers)})

    db = _get_db()
    paper_repo = SQLitePaperRepository(db)
    search_repo = SQLiteSearchRepository(db)

    # Resolve DOIs against existing papers
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
    db.close()

    outcomes = []
    for o in search_record.source_outcomes:
        outcomes.append(
            f"  {o.source}: {o.result_count} results "
            f"({o.status.value}, {o.latency_ms:.0f}ms)"
        )

    return (
        f"Search ID: {search_record.id}\n"
        f"Query: {query}\n"
        f"Sources: {', '.join(source_list)}\n"
        f"Total candidates: {search_record.total_candidates}\n"
        f"Unique papers after dedup: {len(papers)}\n" + "\n".join(outcomes)
    )


@mcp.tool()
def list_searches(limit: int = 20) -> str:
    """List recent search runs with their parameters and results."""
    db = _get_db()
    search_repo = SQLiteSearchRepository(db)
    searches = search_repo.list_searches(limit=limit)
    db.close()

    if not searches:
        return "No search history found."

    lines = []
    for s in searches:
        success = sum(1 for o in s.source_outcomes if o.status.value == "success")
        lines.append(
            f"{s.id[:8]}  {s.created_at:%Y-%m-%d %H:%M}  "
            f'"{s.query}"  {s.total_papers} papers  '
            f"{success}/{len(s.source_outcomes)} sources ok"
        )
    return "\n".join(lines)


@mcp.tool()
def get_papers(search_id: str) -> str:
    """Get all papers from a search, with title, authors, abstract, DOI, and year.

    Supports prefix matching on search IDs.
    """
    db = _get_db()
    search_repo = SQLiteSearchRepository(db)
    paper_repo = SQLitePaperRepository(db)

    # Resolve prefix
    s = search_repo.get(search_id)
    if not s:
        searches = search_repo.list_searches(limit=100)
        matches = [sr for sr in searches if sr.id.startswith(search_id)]
        if len(matches) == 1:
            s = matches[0]
        elif len(matches) > 1:
            db.close()
            return f"Ambiguous prefix '{search_id}'. Matches: {[m.id[:8] for m in matches]}"
        else:
            db.close()
            return f"Search '{search_id}' not found."

    results = search_repo.get_results(s.id)
    paper_ids = {r.paper_id for r in results}
    papers = [p for pid in paper_ids if (p := paper_repo.get(pid))]
    db.close()

    out = [f'Search: {s.id[:8]} — "{s.query}" — {len(papers)} papers\n']
    for i, p in enumerate(papers, 1):
        authors = "; ".join(p.authors[:3])
        if len(p.authors) > 3:
            authors += f" et al. ({len(p.authors)} total)"
        out.append(
            f"[{i}] {p.title}\n"
            f"    Authors: {authors}\n"
            f"    Year: {p.year or 'N/A'}  Journal: {p.journal or 'N/A'}\n"
            f"    DOI: {p.doi or 'N/A'}  ID: {p.id[:8]}\n"
            f"    Abstract: {p.abstract[:300]}{'...' if len(p.abstract) > 300 else ''}\n"
        )
    return "\n".join(out)


@mcp.tool()
def get_paper(paper_id: str) -> str:
    """Get full details of a single paper by ID (supports prefix matching)."""
    db = _get_db()
    paper_repo = SQLitePaperRepository(db)

    paper = paper_repo.get(paper_id)
    if not paper:
        # Try prefix match
        all_papers = paper_repo.list_all(limit=1000)
        matches = [p for p in all_papers if p.id.startswith(paper_id)]
        if len(matches) == 1:
            paper = matches[0]
        elif len(matches) > 1:
            db.close()
            return f"Ambiguous prefix. Matches: {[m.id[:8] for m in matches]}"
        else:
            db.close()
            return f"Paper '{paper_id}' not found."

    db.close()
    return json.dumps(paper.model_dump(mode="json"), indent=2, ensure_ascii=False)


@mcp.tool()
def export_search(
    search_id: str,
    format: str = "json",
) -> str:
    """Export search results as BibTeX, JSON, or CSV.

    Args:
        search_id: Search ID (supports prefix matching)
        format: One of 'bibtex', 'json', 'csv'
    """
    db = _get_db()
    search_repo = SQLiteSearchRepository(db)
    paper_repo = SQLitePaperRepository(db)

    s = search_repo.get(search_id)
    if not s:
        searches = search_repo.list_searches(limit=100)
        matches = [sr for sr in searches if sr.id.startswith(search_id)]
        if len(matches) == 1:
            s = matches[0]
        else:
            db.close()
            return f"Search '{search_id}' not found."

    results = search_repo.get_results(s.id)
    paper_ids = {r.paper_id for r in results}
    papers = [p for pid in paper_ids if (p := paper_repo.get(pid))]
    db.close()

    formatters = {"json": export_json, "csv": export_csv, "bibtex": export_bibtex}
    formatter = formatters.get(format, export_json)
    return formatter(papers)


# -- Research question tools --


@mcp.tool()
def create_question(text: str, description: str = "") -> str:
    """Create a research question for relevance scoring.

    Research questions are first-class entities that papers are scored against.
    Returns the question ID.
    """
    from scitadel.domain.models import ResearchQuestion

    db = _get_db()
    q_repo = SQLiteResearchQuestionRepository(db)

    question = ResearchQuestion(text=text, description=description)
    q_repo.save_question(question)
    db.close()

    return f"Question created: {question.id[:8]}\nText: {text}"


@mcp.tool()
def list_questions() -> str:
    """List all research questions."""
    db = _get_db()
    q_repo = SQLiteResearchQuestionRepository(db)
    questions = q_repo.list_questions()
    db.close()

    if not questions:
        return "No research questions found."

    lines = []
    for q in questions:
        lines.append(f'{q.id[:8]}  {q.created_at:%Y-%m-%d %H:%M}  "{q.text}"')
    return "\n".join(lines)


@mcp.tool()
def add_search_terms(question_id: str, terms: list[str], query_string: str = "") -> str:
    """Add search terms linked to a research question.

    Args:
        question_id: Research question ID (supports prefix)
        terms: List of keywords
        query_string: Optional pre-built query string
    """
    from scitadel.domain.models import SearchTerm

    db = _get_db()
    q_repo = SQLiteResearchQuestionRepository(db)

    # Resolve prefix
    question = q_repo.get_question(question_id)
    if not question:
        questions = q_repo.list_questions()
        matches = [q for q in questions if q.id.startswith(question_id)]
        if len(matches) == 1:
            question = matches[0]
        else:
            db.close()
            return f"Question '{question_id}' not found."

    if not query_string:
        query_string = " ".join(terms)

    term = SearchTerm(
        question_id=question.id,
        terms=terms,
        query_string=query_string,
    )
    q_repo.save_term(term)
    db.close()

    return f"Search terms added to question {question.id[:8]}: {terms}"


# -- Assessment tools --


@mcp.tool()
def assess_paper(
    paper_id: str,
    question_id: str,
    score: float,
    reasoning: str,
    assessor: str = "claude",
    model: str | None = None,
) -> str:
    """Record a relevance assessment for a paper against a research question.

    This tool is designed to be called by an LLM agent after reading a paper's
    abstract and evaluating its relevance to the research question.

    Args:
        paper_id: Paper ID (supports prefix)
        question_id: Research question ID (supports prefix)
        score: Relevance score from 0.0 (irrelevant) to 1.0 (highly relevant)
        reasoning: Explanation of why this score was assigned
        assessor: Who made the assessment (e.g. 'claude', 'human')
        model: Model name if LLM-assessed
    """
    from scitadel.domain.models import Assessment

    db = _get_db()
    paper_repo = SQLitePaperRepository(db)
    q_repo = SQLiteResearchQuestionRepository(db)
    a_repo = SQLiteAssessmentRepository(db)

    # Resolve paper prefix
    paper = paper_repo.get(paper_id)
    if not paper:
        all_papers = paper_repo.list_all(limit=1000)
        matches = [p for p in all_papers if p.id.startswith(paper_id)]
        if len(matches) == 1:
            paper = matches[0]
        else:
            db.close()
            return f"Paper '{paper_id}' not found."

    # Resolve question prefix
    question = q_repo.get_question(question_id)
    if not question:
        questions = q_repo.list_questions()
        matches = [q for q in questions if q.id.startswith(question_id)]
        if len(matches) == 1:
            question = matches[0]
        else:
            db.close()
            return f"Question '{question_id}' not found."

    assessment = Assessment(
        paper_id=paper.id,
        question_id=question.id,
        score=score,
        reasoning=reasoning,
        assessor=assessor,
        model=model,
    )
    a_repo.save(assessment)
    db.close()

    return (
        f"Assessment saved: {assessment.id[:8]}\n"
        f"Paper: {paper.title[:60]}\n"
        f"Question: {question.text[:60]}\n"
        f"Score: {score:.2f}\n"
        f"Reasoning: {reasoning[:200]}"
    )


@mcp.tool()
def get_assessments(
    paper_id: str | None = None,
    question_id: str | None = None,
) -> str:
    """Get relevance assessments, optionally filtered by paper and/or question."""
    db = _get_db()
    a_repo = SQLiteAssessmentRepository(db)
    paper_repo = SQLitePaperRepository(db)

    if paper_id:
        assessments = a_repo.get_for_paper(paper_id, question_id=question_id)
    elif question_id:
        assessments = a_repo.get_for_question(question_id)
    else:
        db.close()
        return "Provide at least one of paper_id or question_id."

    if not assessments:
        db.close()
        return "No assessments found."

    lines = []
    for a in assessments:
        paper = paper_repo.get(a.paper_id)
        title = paper.title[:50] if paper else "Unknown"
        lines.append(
            f"Score: {a.score:.2f}  Paper: {title}  "
            f"Assessor: {a.assessor}  {a.created_at:%Y-%m-%d %H:%M}\n"
            f"  Reasoning: {a.reasoning[:200]}"
        )
    db.close()
    return "\n\n".join(lines)


def main():
    """Run the MCP server."""
    mcp.run()


if __name__ == "__main__":
    main()
