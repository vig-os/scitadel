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
    query: str = "",
    sources: str = "pubmed,arxiv,openalex,inspire",
    max_results: int = 50,
    question_id: str | None = None,
) -> str:
    """Run a federated literature search across scientific databases.

    Searches PubMed, arXiv, OpenAlex, and INSPIRE-HEP in parallel.
    Results are deduplicated and persisted to the local database.
    Returns the search ID and summary statistics.

    Args:
        query: Search query (optional if question_id is provided)
        sources: Comma-separated list of sources
        max_results: Maximum results per source
        question_id: Research question ID — auto-builds query from linked terms
    """
    from scitadel.adapters import build_adapters
    from scitadel.services.orchestrator import run_search

    config = load_config()
    source_list = [s.strip() for s in sources.split(",")]
    parameters: dict = {}

    # Resolve question-driven query
    if question_id:
        db = _get_db()
        q_repo = SQLiteResearchQuestionRepository(db)
        question = q_repo.get_question(question_id)
        if not question:
            questions = q_repo.list_questions()
            matches = [q for q in questions if q.id.startswith(question_id)]
            if len(matches) == 1:
                question = matches[0]
            else:
                db.close()
                return f"Question '{question_id}' not found."
        parameters["question_id"] = question.id
        if not query:
            terms = q_repo.get_terms(question.id)
            if not terms:
                db.close()
                return f"No search terms linked to question '{question.id[:8]}'."
            query = " OR ".join(t.query_string for t in terms if t.query_string)
        db.close()

    if not query:
        return "Provide a query or question_id with linked search terms."

    adapters = build_adapters(
        source_list,
        pubmed_api_key=config.pubmed.api_key,
        openalex_email=config.openalex.api_key,
    )

    search_record, candidates = await run_search(
        query, adapters, max_results=max_results
    )

    papers, search_results = deduplicate(candidates)
    search_record = search_record.model_copy(
        update={
            "total_papers": len(papers),
            "parameters": {**search_record.parameters, **parameters},
        }
    )

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


# -- Snowball tools --


@mcp.tool()
async def snowball_search(
    search_id: str,
    question_id: str,
    depth: int = 1,
    direction: str = "both",
) -> str:
    """Run citation chaining (snowballing) from a search's papers.

    Fetches the citation graph via OpenAlex and persists all discovered
    papers and citation edges. Does NOT score papers — use assess_paper
    on the returned paper IDs to score them after snowballing.

    Args:
        search_id: Search ID to snowball from (supports prefix)
        question_id: Research question to associate with the run
        depth: Max chaining depth (1-3)
        direction: 'references', 'cited_by', or 'both'
    """
    from scitadel.adapters.openalex.citations import (
        OpenAlexCitationFetcher,
        work_to_paper_dict,
    )
    from scitadel.domain.models import Paper
    from scitadel.repositories.sqlite import SQLiteCitationRepository
    from scitadel.services.snowball import SnowballConfig
    from scitadel.services.snowball import snowball as run_snowball

    config = load_config()
    db = _get_db()
    search_repo = SQLiteSearchRepository(db)
    paper_repo = SQLitePaperRepository(db)
    q_repo = SQLiteResearchQuestionRepository(db)
    citation_repo = SQLiteCitationRepository(db)

    # Resolve search
    s = search_repo.get(search_id)
    if not s:
        searches = search_repo.list_searches(limit=100)
        matches = [sr for sr in searches if sr.id.startswith(search_id)]
        if len(matches) == 1:
            s = matches[0]
        else:
            db.close()
            return f"Search '{search_id}' not found."

    # Resolve question
    question = q_repo.get_question(question_id)
    if not question:
        questions = q_repo.list_questions()
        matches = [q for q in questions if q.id.startswith(question_id)]
        if len(matches) == 1:
            question = matches[0]
        else:
            db.close()
            return f"Question '{question_id}' not found."

    # Load seed papers
    results = search_repo.get_results(s.id)
    paper_ids = {r.paper_id for r in results}
    seed_papers = [p for pid in paper_ids if (p := paper_repo.get(pid))]

    fetcher = OpenAlexCitationFetcher(email=config.openalex.api_key)

    class _Resolver:
        def resolve(self, work_dict: dict) -> tuple[Paper, bool]:
            kwargs = work_to_paper_dict(work_dict)
            if kwargs.get("doi"):
                existing = paper_repo.find_by_doi(kwargs["doi"])
                if existing:
                    return existing, False
            if kwargs.get("title"):
                existing = paper_repo.find_by_title(kwargs["title"])
                if existing:
                    return existing, False
            paper = Paper(**kwargs)
            paper_repo.save(paper)
            return paper, True

    snowball_config = SnowballConfig(
        direction=direction,
        max_depth=depth,
    )

    run, citations, new_papers = await run_snowball(
        seed_papers,
        question,
        fetcher=fetcher,
        resolver=_Resolver(),
        scorer=None,
        config=snowball_config,
    )

    run = run.model_copy(update={"search_id": s.id})
    citation_repo.save_many(citations)
    citation_repo.save_snowball_run(run)
    db.close()

    # List new paper IDs so Claude can assess them
    new_ids = [f"  {p.id[:8]}  {p.title[:60]}" for p in new_papers[:20]]
    new_ids_str = "\n".join(new_ids) if new_ids else "  (none)"
    suffix = f"\n  ... and {len(new_papers) - 20} more" if len(new_papers) > 20 else ""

    return (
        f"Snowball run: {run.id[:8]}\n"
        f"Seed papers: {len(seed_papers)}\n"
        f"Discovered: {run.total_discovered}\n"
        f"New papers: {run.total_new_papers}\n"
        f"Citation edges: {len(citations)}\n\n"
        f"New papers (use assess_paper to score):\n{new_ids_str}{suffix}"
    )


# -- Full text tools --


@mcp.tool()
def save_paper_text(
    paper_id: str,
    full_text: str | None = None,
    summary: str | None = None,
) -> str:
    """Save full text and/or summary for a paper.

    Call this after fetching and reading a paper's full text (e.g. from arXiv,
    PMC, or an open-access URL). The summary can be your own condensed version
    of the key findings.

    Args:
        paper_id: Paper ID (supports prefix matching)
        full_text: The paper's full text content
        summary: A summary of the paper's key findings
    """
    db = _get_db()
    paper_repo = SQLitePaperRepository(db)

    paper = paper_repo.get(paper_id)
    if not paper:
        all_papers = paper_repo.list_all(limit=1000)
        matches = [p for p in all_papers if p.id.startswith(paper_id)]
        if len(matches) == 1:
            paper = matches[0]
        else:
            db.close()
            return f"Paper '{paper_id}' not found."

    updates: dict = {}
    if full_text is not None:
        updates["full_text"] = full_text
    if summary is not None:
        updates["summary"] = summary

    if not updates:
        db.close()
        return "Provide at least one of full_text or summary."

    updated = paper.model_copy(update=updates)
    paper_repo.save(updated)
    db.close()

    parts = []
    if full_text is not None:
        parts.append(f"full_text ({len(full_text)} chars)")
    if summary is not None:
        parts.append(f"summary ({len(summary)} chars)")

    return (
        f"Updated paper {paper.id[:8]}: {paper.title[:60]}\n"
        f"Saved: {', '.join(parts)}"
    )


@mcp.tool()
def get_paper_text(paper_id: str) -> str:
    """Get the full text and summary of a paper, if available.

    Returns the stored full text and summary. If no full text is stored,
    returns the abstract and suggests fetching the full text via the paper's
    DOI, arXiv ID, or URL.

    Args:
        paper_id: Paper ID (supports prefix matching)
    """
    db = _get_db()
    paper_repo = SQLitePaperRepository(db)

    paper = paper_repo.get(paper_id)
    if not paper:
        all_papers = paper_repo.list_all(limit=1000)
        matches = [p for p in all_papers if p.id.startswith(paper_id)]
        if len(matches) == 1:
            paper = matches[0]
        else:
            db.close()
            return f"Paper '{paper_id}' not found."

    db.close()

    parts = [f"Paper: {paper.title}", f"ID: {paper.id[:8]}"]

    if paper.summary:
        parts.append(f"\n--- Summary ---\n{paper.summary}")

    if paper.full_text:
        parts.append(f"\n--- Full Text ({len(paper.full_text)} chars) ---")
        parts.append(paper.full_text[:10000])
        if len(paper.full_text) > 10000:
            parts.append(f"\n... truncated ({len(paper.full_text)} total chars)")
    else:
        parts.append("\nNo full text stored.")
        hints = []
        if paper.arxiv_id:
            hints.append(f"  arXiv: https://arxiv.org/abs/{paper.arxiv_id}")
        if paper.doi:
            hints.append(f"  DOI: https://doi.org/{paper.doi}")
        if paper.url:
            hints.append(f"  URL: {paper.url}")
        if hints:
            parts.append("Try fetching from:")
            parts.extend(hints)

    return "\n".join(parts)


@mcp.tool()
async def fetch_paper_text(paper_id: str) -> str:
    """Attempt to fetch the full text of a paper from open-access sources.

    Tries (in order): arXiv, PubMed Central, Unpaywall.
    Stores the text in the database if successful.

    Args:
        paper_id: Paper ID (supports prefix matching)
    """
    import httpx

    db = _get_db()
    paper_repo = SQLitePaperRepository(db)

    paper = paper_repo.get(paper_id)
    if not paper:
        all_papers = paper_repo.list_all(limit=1000)
        matches = [p for p in all_papers if p.id.startswith(paper_id)]
        if len(matches) == 1:
            paper = matches[0]
        else:
            db.close()
            return f"Paper '{paper_id}' not found."

    if paper.full_text:
        db.close()
        return (
            f"Paper {paper.id[:8]} already has full text "
            f"({len(paper.full_text)} chars)."
        )

    text = None
    source_used = ""

    async with httpx.AsyncClient(timeout=30.0, follow_redirects=True) as client:
        # 1. Try arXiv (plain text via ar5iv or HTML)
        if not text and paper.arxiv_id:
            arxiv_id = paper.arxiv_id.replace("arXiv:", "")
            try:
                resp = await client.get(
                    f"https://export.arxiv.org/e-print/{arxiv_id}",
                    headers={"Accept": "text/plain"},
                )
                if resp.status_code == 200 and len(resp.text) > 500:
                    text = resp.text
                    source_used = "arxiv"
            except httpx.HTTPError:
                pass

        # 2. Try PubMed Central (open-access XML → plain text)
        if not text and paper.pubmed_id:
            try:
                # First find the PMC ID
                resp = await client.get(
                    "https://www.ncbi.nlm.nih.gov/pmc/utils/idconv/v1.0/",
                    params={
                        "ids": paper.pubmed_id,
                        "format": "json",
                        "tool": "scitadel",
                    },
                )
                if resp.status_code == 200:
                    data = resp.json()
                    records = data.get("records", [])
                    pmc_id = records[0].get("pmcid") if records else None
                    if pmc_id:
                        resp2 = await client.get(
                            "https://eutils.ncbi.nlm.nih.gov/entrez/eutils"
                            "/efetch.fcgi",
                            params={
                                "db": "pmc",
                                "id": pmc_id,
                                "rettype": "txt",
                                "retmode": "text",
                            },
                        )
                        if resp2.status_code == 200 and len(resp2.text) > 500:
                            text = resp2.text
                            source_used = "pmc"
            except httpx.HTTPError:
                pass

        # 3. Try Unpaywall for open-access PDF URL
        if not text and paper.doi:
            config = load_config()
            email = config.openalex.api_key or "scitadel@example.com"
            try:
                resp = await client.get(
                    f"https://api.unpaywall.org/v2/{paper.doi}",
                    params={"email": email},
                )
                if resp.status_code == 200:
                    data = resp.json()
                    oa_url = None
                    best = data.get("best_oa_location") or {}
                    oa_url = best.get("url_for_pdf") or best.get("url")
                    if oa_url:
                        # Return URL for the agent to fetch, not the PDF itself
                        db.close()
                        return (
                            f"Found open-access link via Unpaywall:\n"
                            f"  URL: {oa_url}\n"
                            f"  Host: {best.get('host_type', 'unknown')}\n"
                            f"  Version: {best.get('version', 'unknown')}\n\n"
                            f"Fetch this URL, extract the text, and use "
                            f"save_paper_text to store it."
                        )
            except httpx.HTTPError:
                pass

    if text:
        updated = paper.model_copy(update={"full_text": text})
        paper_repo.save(updated)
        db.close()
        return (
            f"Fetched full text from {source_used} for paper {paper.id[:8]}.\n"
            f"Stored {len(text)} characters.\n"
            f"Title: {paper.title[:80]}"
        )

    db.close()
    hints = []
    if paper.doi:
        hints.append(f"  DOI: https://doi.org/{paper.doi}")
    if paper.arxiv_id:
        hints.append(f"  arXiv: https://arxiv.org/abs/{paper.arxiv_id}")
    return (
        f"Could not automatically fetch full text for {paper.id[:8]}.\n"
        f"Try manually fetching from:\n" + "\n".join(hints)
    )


def main():
    """Run the MCP server."""
    mcp.run()


if __name__ == "__main__":
    main()
