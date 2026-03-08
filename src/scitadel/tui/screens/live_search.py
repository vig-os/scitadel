"""Live search — tab widget to run a search and watch progress."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Vertical
from textual.widgets import Button, DataTable, Input, Log, Static


class LiveSearch(Vertical):
    """Tab content: run a new search and watch results stream in."""

    DEFAULT_CSS = """
    LiveSearch {
        height: 1fr;
    }
    LiveSearch > Static {
        height: 1;
        background: $primary-darken-2;
        color: $text;
        padding: 0 1;
    }
    #search-input {
        height: 3;
    }
    #search-log {
        height: 10;
        border: solid $primary;
    }
    #search-btn {
        width: 16;
    }
    """

    def compose(self) -> ComposeResult:
        yield Static("New Search — enter query and press Search")
        yield Input(placeholder="Enter search query...", id="search-input")
        yield Button("Search", id="search-btn", variant="primary")
        yield Log(id="search-log")
        yield DataTable(id="search-results-table")

    def on_mount(self) -> None:
        table = self.query_one("#search-results-table", DataTable)
        table.add_columns("ID", "Year", "Title", "DOI")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "search-btn":
            query = self.query_one("#search-input", Input).value.strip()
            if query:
                self._run_search(query)

    def _run_search(self, query: str) -> None:
        self.run_worker(self._do_search(query), thread=False)

    async def _do_search(self, query: str) -> None:
        log = self.query_one("#search-log", Log)
        table = self.query_one("#search-results-table", DataTable)
        table.clear()

        log.write_line(f"Searching for: {query}")

        from scitadel.adapters import build_adapters
        from scitadel.config import load_config
        from scitadel.services.dedup import deduplicate
        from scitadel.services.orchestrator import run_search

        config = load_config()
        adapters = build_adapters(
            list(config.default_sources),
            pubmed_api_key=config.pubmed.api_key,
            openalex_email=config.openalex.api_key,
        )

        log.write_line(f"  Querying {len(adapters)} sources...")

        search_record, candidates = await run_search(query, adapters)

        for outcome in search_record.source_outcomes:
            icon = "+" if outcome.status.value == "success" else "!"
            log.write_line(
                f"  [{icon}] {outcome.source}: "
                f"{outcome.result_count} results ({outcome.latency_ms:.0f}ms)"
            )

        papers, search_results = deduplicate(candidates)
        search_record = search_record.model_copy(
            update={"total_papers": len(papers)}
        )
        log.write_line(f"  Unique papers: {len(papers)}")

        # Persist
        store = self.app.store
        from scitadel.repositories.sqlite import (
            SQLitePaperRepository,
            SQLiteSearchRepository,
        )

        paper_repo = SQLitePaperRepository(store._db)
        search_repo = SQLiteSearchRepository(store._db)

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

        log.write_line(f"  Search ID: {search_record.id[:8]}")

        # Populate results table
        for p in papers[:50]:
            table.add_row(
                p.id[:8],
                str(p.year or ""),
                p.title[:60],
                p.doi or "",
            )
