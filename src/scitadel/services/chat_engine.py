"""Chat engine — async agentic loop with tool dispatch.

Manages a conversation with Claude, executing tools in-process via the
same DataStore/repos that the TUI uses. Yields streaming events for the UI.
"""

from __future__ import annotations

import json
import logging
from collections.abc import AsyncIterator
from dataclasses import dataclass, field

import anthropic

from scitadel.config import load_config
from scitadel.domain.models import (
    Assessment,
    Paper,
    ResearchQuestion,
    SearchTerm,
)
from scitadel.services.dedup import deduplicate
from scitadel.services.export import export_bibtex, export_csv, export_json
from scitadel.services.tool_defs import SYSTEM_PROMPT, TOOLS
from scitadel.tui.data import DataStore

logger = logging.getLogger(__name__)


class MissingAPIKeyError(Exception):
    """Raised when no Anthropic API key is configured."""

    pass


# -- Chat events --


@dataclass
class TextDelta:
    """A chunk of assistant text."""

    text: str


@dataclass
class ToolCallStart:
    """A tool call has begun."""

    tool_name: str
    tool_id: str
    arguments: dict


@dataclass
class ToolCallResult:
    """A tool call has finished."""

    tool_name: str
    tool_id: str
    result: str


@dataclass
class TurnComplete:
    """The assistant has finished its turn (no more tool calls)."""

    pass


ChatEvent = TextDelta | ToolCallStart | ToolCallResult | TurnComplete


# -- Tool dispatcher --


class ToolDispatcher:
    """Executes tool calls against the shared DataStore."""

    def __init__(self, store: DataStore) -> None:
        self._store = store

    async def dispatch(self, tool_name: str, arguments: dict) -> str:
        """Execute a tool call and return the string result."""
        handler = getattr(self, f"_tool_{tool_name}", None)
        if not handler:
            return f"Unknown tool: {tool_name}"
        try:
            result = await handler(**arguments)
            return result
        except Exception as exc:
            logger.exception("Tool %s failed", tool_name)
            return f"Tool error: {exc}"

    async def _tool_search(
        self,
        query: str = "",
        sources: str = "pubmed,arxiv,openalex,inspire",
        max_results: int = 50,
        question_id: str | None = None,
    ) -> str:
        from scitadel.adapters import build_adapters
        from scitadel.services.orchestrator import run_search

        config = load_config()
        source_list = [s.strip() for s in sources.split(",")]
        parameters: dict = {}

        if question_id:
            resolved = self._store.resolve_prefix_id("question", question_id)
            if not resolved:
                return f"Question '{question_id}' not found."
            question_id = resolved
            question = self._store.get_question(question_id)
            parameters["question_id"] = question_id
            if not query and question:
                terms = self._store.get_terms(question_id)
                if not terms:
                    return f"No search terms linked to question '{question_id[:8]}'."
                query = " OR ".join(t.query_string for t in terms if t.query_string)

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

        # Resolve DOIs against existing papers
        id_map: dict[str, str] = {}
        for paper in papers:
            if paper.doi:
                existing = self._store.find_paper_by_doi(paper.doi)
                if existing and existing.id != paper.id:
                    id_map[paper.id] = existing.id
                    paper.id = existing.id

        self._store.save_papers(papers)
        self._store.save_search(search_record)

        for sr in search_results:
            sr.search_id = search_record.id
            sr.paper_id = id_map.get(sr.paper_id, sr.paper_id)
        self._store.save_search_results(search_results)

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

    async def _tool_list_searches(self, limit: int = 20) -> str:
        searches = self._store.list_searches(limit=limit)
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

    async def _tool_get_papers(self, search_id: str) -> str:
        resolved = self._store.resolve_prefix_id("search", search_id)
        if not resolved:
            return f"Search '{search_id}' not found."
        search_id = resolved

        s = self._store.get_search(search_id)
        papers = self._store.get_papers_for_search(search_id)

        out = [f'Search: {search_id[:8]} — "{s.query}" — {len(papers)} papers\n']
        for i, p in enumerate(papers, 1):
            authors = "; ".join(p.authors[:3])
            if len(p.authors) > 3:
                authors += f" et al. ({len(p.authors)} total)"
            out.append(
                f"[{i}] {p.title}\n"
                f"    Authors: {authors}\n"
                f"    Year: {p.year or 'N/A'}  "
                f"Journal: {p.journal or 'N/A'}\n"
                f"    DOI: {p.doi or 'N/A'}  ID: {p.id[:8]}\n"
                f"    Abstract: {p.abstract[:300]}"
                f"{'...' if len(p.abstract) > 300 else ''}\n"
            )
        return "\n".join(out)

    async def _tool_get_paper(self, paper_id: str) -> str:
        resolved = self._store.resolve_prefix_id("paper", paper_id)
        if not resolved:
            return f"Paper '{paper_id}' not found."

        paper = self._store.get_paper(resolved)
        if not paper:
            return f"Paper '{paper_id}' not found."
        return json.dumps(paper.model_dump(mode="json"), indent=2, ensure_ascii=False)

    async def _tool_export_search(self, search_id: str, format: str = "json") -> str:
        resolved = self._store.resolve_prefix_id("search", search_id)
        if not resolved:
            return f"Search '{search_id}' not found."

        papers = self._store.get_papers_for_search(resolved)
        formatters = {
            "json": export_json,
            "csv": export_csv,
            "bibtex": export_bibtex,
        }
        formatter = formatters.get(format, export_json)
        return formatter(papers)

    async def _tool_create_question(self, text: str, description: str = "") -> str:
        question = ResearchQuestion(text=text, description=description)
        self._store.save_question(question)
        return f"Question created: {question.id[:8]}\nText: {text}"

    async def _tool_list_questions(self) -> str:
        questions = self._store.list_questions()
        if not questions:
            return "No research questions found."

        lines = []
        for q in questions:
            lines.append(f'{q.id[:8]}  {q.created_at:%Y-%m-%d %H:%M}  "{q.text}"')
        return "\n".join(lines)

    async def _tool_add_search_terms(
        self,
        question_id: str,
        terms: list[str],
        query_string: str = "",
    ) -> str:
        resolved = self._store.resolve_prefix_id("question", question_id)
        if not resolved:
            return f"Question '{question_id}' not found."

        if not query_string:
            query_string = " ".join(terms)

        term = SearchTerm(
            question_id=resolved,
            terms=terms,
            query_string=query_string,
        )
        self._store.save_term(term)
        return f"Search terms added to question {resolved[:8]}: {terms}"

    async def _tool_assess_paper(
        self,
        paper_id: str,
        question_id: str,
        score: float,
        reasoning: str,
        assessor: str = "claude",
        model: str | None = None,
    ) -> str:
        paper_resolved = self._store.resolve_prefix_id("paper", paper_id)
        if not paper_resolved:
            return f"Paper '{paper_id}' not found."

        q_resolved = self._store.resolve_prefix_id("question", question_id)
        if not q_resolved:
            return f"Question '{question_id}' not found."

        paper = self._store.get_paper(paper_resolved)
        question = self._store.get_question(q_resolved)

        assessment = Assessment(
            paper_id=paper_resolved,
            question_id=q_resolved,
            score=score,
            reasoning=reasoning,
            assessor=assessor,
            model=model,
        )
        self._store.save_assessment(assessment)

        title = paper.title[:60] if paper else "Unknown"
        q_text = question.text[:60] if question else "Unknown"
        return (
            f"Assessment saved: {assessment.id[:8]}\n"
            f"Paper: {title}\n"
            f"Question: {q_text}\n"
            f"Score: {score:.2f}\n"
            f"Reasoning: {reasoning[:200]}"
        )

    async def _tool_get_assessments(
        self,
        paper_id: str | None = None,
        question_id: str | None = None,
    ) -> str:
        if paper_id:
            assessments = self._store.get_assessments_for_paper(
                paper_id, question_id=question_id
            )
        elif question_id:
            assessments = self._store.get_assessments_for_question(question_id)
        else:
            return "Provide at least one of paper_id or question_id."

        if not assessments:
            return "No assessments found."

        lines = []
        for a in assessments:
            paper = self._store.get_paper(a.paper_id)
            title = paper.title[:50] if paper else "Unknown"
            lines.append(
                f"Score: {a.score:.2f}  Paper: {title}  "
                f"Assessor: {a.assessor}  {a.created_at:%Y-%m-%d %H:%M}\n"
                f"  Reasoning: {a.reasoning[:200]}"
            )
        return "\n\n".join(lines)

    async def _tool_snowball_search(
        self,
        search_id: str,
        question_id: str,
        depth: int = 1,
        threshold: float = 0.6,
        direction: str = "both",
        model: str = "claude-sonnet-4-6",
    ) -> str:
        from scitadel.adapters.openalex.citations import (
            OpenAlexCitationFetcher,
            work_to_paper_dict,
        )
        from scitadel.services.scoring import ScoringConfig, score_paper
        from scitadel.services.snowball import SnowballConfig
        from scitadel.services.snowball import snowball as run_snowball

        config = load_config()

        s_resolved = self._store.resolve_prefix_id("search", search_id)
        if not s_resolved:
            return f"Search '{search_id}' not found."

        q_resolved = self._store.resolve_prefix_id("question", question_id)
        if not q_resolved:
            return f"Question '{question_id}' not found."

        question = self._store.get_question(q_resolved)
        seed_papers = self._store.get_papers_for_search(s_resolved)

        fetcher = OpenAlexCitationFetcher(email=config.openalex.api_key)
        scoring_config = ScoringConfig(model=model)
        sync_client = anthropic.Anthropic()
        store = self._store

        class _Resolver:
            def resolve(self, work_dict: dict) -> tuple[Paper, bool]:
                kwargs = work_to_paper_dict(work_dict)
                if kwargs.get("doi"):
                    existing = store.find_paper_by_doi(kwargs["doi"])
                    if existing:
                        return existing, False
                if kwargs.get("title"):
                    existing = store.find_paper_by_title(kwargs["title"])
                    if existing:
                        return existing, False
                paper = Paper(**kwargs)
                store.save_paper(paper)
                return paper, True

        class _Scorer:
            def score(self, paper: Paper, q: ResearchQuestion) -> float:
                assessment = score_paper(
                    paper, q, config=scoring_config, client=sync_client
                )
                return assessment.score

        snowball_config = SnowballConfig(
            direction=direction,
            max_depth=depth,
            threshold=threshold,
        )

        run, citations, new_papers = await run_snowball(
            seed_papers,
            question,
            fetcher=fetcher,
            resolver=_Resolver(),
            scorer=_Scorer(),
            config=snowball_config,
        )

        run = run.model_copy(update={"search_id": s_resolved})
        self._store.save_citations(citations)
        self._store.save_snowball_run(run)

        return (
            f"Snowball run: {run.id[:8]}\n"
            f"Seed papers: {len(seed_papers)}\n"
            f"Discovered: {run.total_discovered}\n"
            f"New papers: {run.total_new_papers}\n"
            f"Citation edges: {len(citations)}"
        )


# -- Chat engine --


MAX_HISTORY_MESSAGES = 40


def _trim_messages(messages: list[dict], max_messages: int) -> list[dict]:
    """Trim message history with a sliding window.

    Never splits tool_use/tool_result pairs: if trimming would land between
    an assistant message containing tool_use and the following user message
    with tool_results, we keep both.
    """
    if len(messages) <= max_messages:
        return messages

    start = len(messages) - max_messages
    trimmed = messages[start:]

    # If the first message is a user message containing tool_results,
    # it's the second half of a tool_use/tool_result pair — include the
    # preceding assistant message too.
    if trimmed and trimmed[0].get("role") == "user":
        content = trimmed[0].get("content")
        if isinstance(content, list) and any(
            isinstance(c, dict) and c.get("type") == "tool_result" for c in content
        ):
            if start > 0:
                trimmed.insert(0, messages[start - 1])

    return trimmed


@dataclass
class ChatEngine:
    """Async agentic loop managing conversation with Claude.

    Processes user messages, dispatches tool calls in-process,
    and yields streaming events for the UI to render.
    """

    store: DataStore
    model: str = "claude-sonnet-4-6"
    max_tokens: int = 4096
    max_history_messages: int = MAX_HISTORY_MESSAGES
    _client: anthropic.AsyncAnthropic | None = field(default=None, init=False)
    _messages: list[dict] = field(default_factory=list, init=False)
    _dispatcher: ToolDispatcher = field(init=False)

    def __post_init__(self) -> None:
        self._dispatcher = ToolDispatcher(self.store)

    def _get_client(self) -> anthropic.AsyncAnthropic:
        """Lazily create the Anthropic client on first use.

        Checks env vars first, then falls back to the system keyring.
        """
        if self._client is None:
            from scitadel.secrets import get_api_key

            api_key = get_api_key()
            if not api_key:
                raise MissingAPIKeyError("No Anthropic API key found.")
            self._client = anthropic.AsyncAnthropic(api_key=api_key)
        return self._client

    async def send(self, user_message: str) -> AsyncIterator[ChatEvent]:
        """Send a user message and yield events as the assistant responds.

        Implements the agentic loop: keeps calling the API until the
        assistant stops requesting tool calls.
        """
        self._messages.append({"role": "user", "content": user_message})

        while True:
            self._messages = _trim_messages(self._messages, self.max_history_messages)
            response = await self._get_client().messages.create(
                model=self.model,
                max_tokens=self.max_tokens,
                system=SYSTEM_PROMPT,
                tools=TOOLS,
                messages=self._messages,
            )

            # Build the assistant message content list
            assistant_content: list[dict] = []
            tool_calls: list[dict] = []

            for block in response.content:
                if block.type == "text":
                    assistant_content.append({"type": "text", "text": block.text})
                    yield TextDelta(text=block.text)
                elif block.type == "tool_use":
                    assistant_content.append(
                        {
                            "type": "tool_use",
                            "id": block.id,
                            "name": block.name,
                            "input": block.input,
                        }
                    )
                    tool_calls.append(
                        {
                            "id": block.id,
                            "name": block.name,
                            "input": block.input,
                        }
                    )

            # Append assistant message
            self._messages.append({"role": "assistant", "content": assistant_content})

            # If no tool calls, we're done
            if response.stop_reason != "tool_use":
                yield TurnComplete()
                return

            # Execute tool calls and build tool result message
            tool_results: list[dict] = []
            for tc in tool_calls:
                yield ToolCallStart(
                    tool_name=tc["name"],
                    tool_id=tc["id"],
                    arguments=tc["input"],
                )

                result = await self._dispatcher.dispatch(tc["name"], tc["input"])

                yield ToolCallResult(
                    tool_name=tc["name"],
                    tool_id=tc["id"],
                    result=result,
                )

                tool_results.append(
                    {
                        "type": "tool_result",
                        "tool_use_id": tc["id"],
                        "content": result,
                    }
                )

            # Append tool results and loop
            self._messages.append({"role": "user", "content": tool_results})
