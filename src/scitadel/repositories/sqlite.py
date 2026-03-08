"""SQLite repository implementations.

Concrete implementations of the repository port interfaces.
All SQL is contained within this module — no raw SQL leaks into services.
"""

from __future__ import annotations

import json
import sqlite3
from datetime import datetime, timezone
from importlib import resources
from pathlib import Path

from scitadel.domain.models import (
    Assessment,
    Citation,
    CitationDirection,
    Paper,
    ResearchQuestion,
    Search,
    SearchResult,
    SearchTerm,
    SnowballRun,
    SourceOutcome,
    SourceStatus,
)


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


class Database:
    """SQLite connection manager with migration support."""

    def __init__(self, db_path: str | Path = ":memory:") -> None:
        self.db_path = str(db_path)
        self._conn: sqlite3.Connection | None = None

    @property
    def conn(self) -> sqlite3.Connection:
        if self._conn is None:
            if self.db_path != ":memory:":
                Path(self.db_path).parent.mkdir(parents=True, exist_ok=True)
            self._conn = sqlite3.connect(self.db_path)
            self._conn.row_factory = sqlite3.Row
            self._conn.execute("PRAGMA journal_mode=WAL")
            self._conn.execute("PRAGMA foreign_keys=ON")
        return self._conn

    def migrate(self) -> None:
        """Apply all pending migrations."""
        migration_dir = resources.files("scitadel.repositories") / "migrations"
        migration_files = sorted(
            f for f in migration_dir.iterdir() if f.name.endswith(".sql")
        )

        for migration_file in migration_files:
            sql = migration_file.read_text()
            self.conn.executescript(sql)
        self.conn.commit()

    def close(self) -> None:
        if self._conn is not None:
            self._conn.close()
            self._conn = None


class SQLitePaperRepository:
    """SQLite implementation of PaperRepository."""

    def __init__(self, db: Database) -> None:
        self._db = db

    _UPSERT_SQL = """\
        INSERT INTO papers
            (id, title, authors, abstract, full_text, summary, doi, arxiv_id,
             pubmed_id, inspire_id, openalex_id, year, journal, url,
             source_urls, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            title      = excluded.title,
            authors    = excluded.authors,
            abstract   = CASE WHEN excluded.abstract != '' THEN excluded.abstract
                              ELSE papers.abstract END,
            full_text  = COALESCE(excluded.full_text, papers.full_text),
            summary    = COALESCE(excluded.summary, papers.summary),
            doi        = COALESCE(excluded.doi, papers.doi),
            arxiv_id   = COALESCE(excluded.arxiv_id, papers.arxiv_id),
            pubmed_id  = COALESCE(excluded.pubmed_id, papers.pubmed_id),
            inspire_id = COALESCE(excluded.inspire_id, papers.inspire_id),
            openalex_id= COALESCE(excluded.openalex_id, papers.openalex_id),
            year       = COALESCE(excluded.year, papers.year),
            journal    = COALESCE(excluded.journal, papers.journal),
            url        = COALESCE(excluded.url, papers.url),
            source_urls= excluded.source_urls,
            updated_at = excluded.updated_at"""

    def _paper_row(self, paper: Paper) -> tuple:
        return (
            paper.id,
            paper.title,
            json.dumps(paper.authors),
            paper.abstract,
            paper.full_text,
            paper.summary,
            paper.doi,
            paper.arxiv_id,
            paper.pubmed_id,
            paper.inspire_id,
            paper.openalex_id,
            paper.year,
            paper.journal,
            paper.url,
            json.dumps(paper.source_urls),
            paper.created_at.isoformat(),
            paper.updated_at.isoformat(),
        )

    def save(self, paper: Paper) -> None:
        self._db.conn.execute(self._UPSERT_SQL, self._paper_row(paper))
        self._db.conn.commit()

    def save_many(self, papers: list[Paper]) -> None:
        for paper in papers:
            self._db.conn.execute(self._UPSERT_SQL, self._paper_row(paper))
        self._db.conn.commit()

    def get(self, paper_id: str) -> Paper | None:
        row = self._db.conn.execute(
            "SELECT * FROM papers WHERE id = ?", (paper_id,)
        ).fetchone()
        return _row_to_paper(row) if row else None

    def find_by_doi(self, doi: str) -> Paper | None:
        row = self._db.conn.execute(
            "SELECT * FROM papers WHERE doi = ?", (doi,)
        ).fetchone()
        return _row_to_paper(row) if row else None

    def find_by_title(self, title: str, threshold: float = 0.85) -> Paper | None:
        # Simple exact match for now; fuzzy matching added in #11
        row = self._db.conn.execute(
            "SELECT * FROM papers WHERE LOWER(title) = LOWER(?)", (title,)
        ).fetchone()
        return _row_to_paper(row) if row else None

    def list_all(self, limit: int = 100, offset: int = 0) -> list[Paper]:
        rows = self._db.conn.execute(
            "SELECT * FROM papers ORDER BY created_at DESC LIMIT ? OFFSET ?",
            (limit, offset),
        ).fetchall()
        return [_row_to_paper(r) for r in rows]


class SQLiteSearchRepository:
    """SQLite implementation of SearchRepository."""

    def __init__(self, db: Database) -> None:
        self._db = db

    def save(self, search: Search) -> None:
        outcomes_json = json.dumps([o.model_dump() for o in search.source_outcomes])
        self._db.conn.execute(
            """INSERT INTO searches
               (id, query, sources, parameters, source_outcomes,
                total_candidates, total_papers, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
                query = excluded.query,
                sources = excluded.sources,
                parameters = excluded.parameters,
                source_outcomes = excluded.source_outcomes,
                total_candidates = excluded.total_candidates,
                total_papers = excluded.total_papers""",
            (
                search.id,
                search.query,
                json.dumps(search.sources),
                json.dumps(search.parameters),
                outcomes_json,
                search.total_candidates,
                search.total_papers,
                search.created_at.isoformat(),
            ),
        )
        self._db.conn.commit()

    def get(self, search_id: str) -> Search | None:
        row = self._db.conn.execute(
            "SELECT * FROM searches WHERE id = ?", (search_id,)
        ).fetchone()
        return _row_to_search(row) if row else None

    def save_results(self, results: list[SearchResult]) -> None:
        for r in results:
            self._db.conn.execute(
                """INSERT INTO search_results
                   (search_id, paper_id, source, rank, score, raw_metadata)
                   VALUES (?, ?, ?, ?, ?, ?)
                   ON CONFLICT(search_id, paper_id, source) DO UPDATE SET
                    rank = excluded.rank,
                    score = excluded.score,
                    raw_metadata = excluded.raw_metadata""",
                (
                    r.search_id,
                    r.paper_id,
                    r.source,
                    r.rank,
                    r.score,
                    json.dumps(r.raw_metadata),
                ),
            )
        self._db.conn.commit()

    def get_results(self, search_id: str) -> list[SearchResult]:
        rows = self._db.conn.execute(
            "SELECT * FROM search_results WHERE search_id = ?", (search_id,)
        ).fetchall()
        return [
            SearchResult(
                search_id=r["search_id"],
                paper_id=r["paper_id"],
                source=r["source"],
                rank=r["rank"],
                score=r["score"],
                raw_metadata=json.loads(r["raw_metadata"]),
            )
            for r in rows
        ]

    def list_searches(self, limit: int = 20) -> list[Search]:
        rows = self._db.conn.execute(
            "SELECT * FROM searches ORDER BY created_at DESC LIMIT ?", (limit,)
        ).fetchall()
        return [_row_to_search(r) for r in rows]

    def diff_searches(
        self, search_id_a: str, search_id_b: str
    ) -> tuple[list[str], list[str]]:
        """Return (added_paper_ids, removed_paper_ids) between two runs."""
        papers_a = {
            r["paper_id"]
            for r in self._db.conn.execute(
                "SELECT DISTINCT paper_id FROM search_results WHERE search_id = ?",
                (search_id_a,),
            ).fetchall()
        }
        papers_b = {
            r["paper_id"]
            for r in self._db.conn.execute(
                "SELECT DISTINCT paper_id FROM search_results WHERE search_id = ?",
                (search_id_b,),
            ).fetchall()
        }
        added = sorted(papers_b - papers_a)
        removed = sorted(papers_a - papers_b)
        return added, removed


class SQLiteResearchQuestionRepository:
    """SQLite implementation of ResearchQuestionRepository."""

    def __init__(self, db: Database) -> None:
        self._db = db

    def save_question(self, question: ResearchQuestion) -> None:
        self._db.conn.execute(
            """INSERT OR REPLACE INTO research_questions
               (id, text, description, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?)""",
            (
                question.id,
                question.text,
                question.description,
                question.created_at.isoformat(),
                question.updated_at.isoformat(),
            ),
        )
        self._db.conn.commit()

    def get_question(self, question_id: str) -> ResearchQuestion | None:
        row = self._db.conn.execute(
            "SELECT * FROM research_questions WHERE id = ?", (question_id,)
        ).fetchone()
        if not row:
            return None
        return ResearchQuestion(
            id=row["id"],
            text=row["text"],
            description=row["description"],
            created_at=datetime.fromisoformat(row["created_at"]),
            updated_at=datetime.fromisoformat(row["updated_at"]),
        )

    def list_questions(self) -> list[ResearchQuestion]:
        rows = self._db.conn.execute(
            "SELECT * FROM research_questions ORDER BY created_at DESC"
        ).fetchall()
        return [
            ResearchQuestion(
                id=r["id"],
                text=r["text"],
                description=r["description"],
                created_at=datetime.fromisoformat(r["created_at"]),
                updated_at=datetime.fromisoformat(r["updated_at"]),
            )
            for r in rows
        ]

    def save_term(self, term: SearchTerm) -> None:
        self._db.conn.execute(
            """INSERT OR REPLACE INTO search_terms
               (id, question_id, terms, query_string, created_at)
               VALUES (?, ?, ?, ?, ?)""",
            (
                term.id,
                term.question_id,
                json.dumps(term.terms),
                term.query_string,
                term.created_at.isoformat(),
            ),
        )
        self._db.conn.commit()

    def get_terms(self, question_id: str) -> list[SearchTerm]:
        rows = self._db.conn.execute(
            "SELECT * FROM search_terms WHERE question_id = ?", (question_id,)
        ).fetchall()
        return [
            SearchTerm(
                id=r["id"],
                question_id=r["question_id"],
                terms=json.loads(r["terms"]),
                query_string=r["query_string"],
                created_at=datetime.fromisoformat(r["created_at"]),
            )
            for r in rows
        ]


class SQLiteAssessmentRepository:
    """SQLite implementation of AssessmentRepository."""

    def __init__(self, db: Database) -> None:
        self._db = db

    def save(self, assessment: Assessment) -> None:
        self._db.conn.execute(
            """INSERT OR REPLACE INTO assessments
               (id, paper_id, question_id, score, reasoning, model,
                prompt, temperature, assessor, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                assessment.id,
                assessment.paper_id,
                assessment.question_id,
                assessment.score,
                assessment.reasoning,
                assessment.model,
                assessment.prompt,
                assessment.temperature,
                assessment.assessor,
                assessment.created_at.isoformat(),
            ),
        )
        self._db.conn.commit()

    def get_for_paper(
        self, paper_id: str, question_id: str | None = None
    ) -> list[Assessment]:
        if question_id:
            rows = self._db.conn.execute(
                "SELECT * FROM assessments WHERE paper_id = ? AND question_id = ?",
                (paper_id, question_id),
            ).fetchall()
        else:
            rows = self._db.conn.execute(
                "SELECT * FROM assessments WHERE paper_id = ?", (paper_id,)
            ).fetchall()
        return [_row_to_assessment(r) for r in rows]

    def get_for_question(self, question_id: str) -> list[Assessment]:
        rows = self._db.conn.execute(
            "SELECT * FROM assessments WHERE question_id = ?", (question_id,)
        ).fetchall()
        return [_row_to_assessment(r) for r in rows]


class SQLiteCitationRepository:
    """SQLite implementation of CitationRepository."""

    def __init__(self, db: Database) -> None:
        self._db = db

    _UPSERT_SQL = """\
        INSERT INTO citations
            (source_paper_id, target_paper_id, direction,
             discovered_by, depth, snowball_run_id)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(source_paper_id, target_paper_id, direction) DO UPDATE SET
            depth = MIN(citations.depth, excluded.depth),
            snowball_run_id = COALESCE(excluded.snowball_run_id,
                                       citations.snowball_run_id)"""

    def _citation_row(self, c: Citation) -> tuple:
        return (
            c.source_paper_id,
            c.target_paper_id,
            c.direction.value,
            c.discovered_by,
            c.depth,
            c.snowball_run_id,
        )

    def save(self, citation: Citation) -> None:
        self._db.conn.execute(self._UPSERT_SQL, self._citation_row(citation))
        self._db.conn.commit()

    def save_many(self, citations: list[Citation]) -> None:
        for c in citations:
            self._db.conn.execute(self._UPSERT_SQL, self._citation_row(c))
        self._db.conn.commit()

    def get_references(self, paper_id: str) -> list[Citation]:
        rows = self._db.conn.execute(
            "SELECT * FROM citations WHERE source_paper_id = ? AND direction = ?",
            (paper_id, CitationDirection.REFERENCES.value),
        ).fetchall()
        return [_row_to_citation(r) for r in rows]

    def get_citations(self, paper_id: str) -> list[Citation]:
        rows = self._db.conn.execute(
            "SELECT * FROM citations WHERE target_paper_id = ? AND direction = ?",
            (paper_id, CitationDirection.CITED_BY.value),
        ).fetchall()
        return [_row_to_citation(r) for r in rows]

    def exists(
        self, source_paper_id: str, target_paper_id: str, direction: str
    ) -> bool:
        row = self._db.conn.execute(
            "SELECT 1 FROM citations WHERE source_paper_id = ? "
            "AND target_paper_id = ? AND direction = ?",
            (source_paper_id, target_paper_id, direction),
        ).fetchone()
        return row is not None

    def save_snowball_run(self, run: SnowballRun) -> None:
        self._db.conn.execute(
            """INSERT OR REPLACE INTO snowball_runs
               (id, search_id, question_id, direction, max_depth,
                threshold, total_discovered, total_new_papers, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                run.id,
                run.search_id,
                run.question_id,
                run.direction,
                run.max_depth,
                run.threshold,
                run.total_discovered,
                run.total_new_papers,
                run.created_at.isoformat(),
            ),
        )
        self._db.conn.commit()

    def get_snowball_run(self, run_id: str) -> SnowballRun | None:
        row = self._db.conn.execute(
            "SELECT * FROM snowball_runs WHERE id = ?", (run_id,)
        ).fetchone()
        return _row_to_snowball_run(row) if row else None

    def list_snowball_runs(self, limit: int = 20) -> list[SnowballRun]:
        rows = self._db.conn.execute(
            "SELECT * FROM snowball_runs ORDER BY created_at DESC LIMIT ?", (limit,)
        ).fetchall()
        return [_row_to_snowball_run(r) for r in rows]


# -- Row mapping helpers --


def _row_to_paper(row: sqlite3.Row) -> Paper:
    return Paper(
        id=row["id"],
        title=row["title"],
        authors=json.loads(row["authors"]),
        abstract=row["abstract"],
        full_text=row["full_text"],
        summary=row["summary"],
        doi=row["doi"],
        arxiv_id=row["arxiv_id"],
        pubmed_id=row["pubmed_id"],
        inspire_id=row["inspire_id"],
        openalex_id=row["openalex_id"],
        year=row["year"],
        journal=row["journal"],
        url=row["url"],
        source_urls=json.loads(row["source_urls"]),
        created_at=datetime.fromisoformat(row["created_at"]),
        updated_at=datetime.fromisoformat(row["updated_at"]),
    )


def _row_to_search(row: sqlite3.Row) -> Search:
    raw_outcomes = json.loads(row["source_outcomes"])
    outcomes = [
        SourceOutcome(
            source=o["source"],
            status=SourceStatus(o["status"]),
            result_count=o.get("result_count", 0),
            latency_ms=o.get("latency_ms", 0.0),
            error=o.get("error"),
        )
        for o in raw_outcomes
    ]
    return Search(
        id=row["id"],
        query=row["query"],
        sources=json.loads(row["sources"]),
        parameters=json.loads(row["parameters"]),
        source_outcomes=outcomes,
        total_candidates=row["total_candidates"],
        total_papers=row["total_papers"],
        created_at=datetime.fromisoformat(row["created_at"]),
    )


def _row_to_assessment(row: sqlite3.Row) -> Assessment:
    return Assessment(
        id=row["id"],
        paper_id=row["paper_id"],
        question_id=row["question_id"],
        score=row["score"],
        reasoning=row["reasoning"],
        model=row["model"],
        prompt=row["prompt"],
        temperature=row["temperature"],
        assessor=row["assessor"],
        created_at=datetime.fromisoformat(row["created_at"]),
    )


def _row_to_citation(row: sqlite3.Row) -> Citation:
    return Citation(
        source_paper_id=row["source_paper_id"],
        target_paper_id=row["target_paper_id"],
        direction=CitationDirection(row["direction"]),
        discovered_by=row["discovered_by"],
        depth=row["depth"],
        snowball_run_id=row["snowball_run_id"],
    )


def _row_to_snowball_run(row: sqlite3.Row) -> SnowballRun:
    return SnowballRun(
        id=row["id"],
        search_id=row["search_id"],
        question_id=row["question_id"],
        direction=row["direction"],
        max_depth=row["max_depth"],
        threshold=row["threshold"],
        total_discovered=row["total_discovered"],
        total_new_papers=row["total_new_papers"],
        created_at=datetime.fromisoformat(row["created_at"]),
    )
