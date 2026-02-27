"""DB-backed export service (BibTeX, JSON, CSV).

Exports operate on persisted canonical records only —
no direct export from transient in-memory adapter responses.
"""

from __future__ import annotations

import csv
import io
import json
import re

from scitadel.domain.models import Paper


def export_json(papers: list[Paper], indent: int = 2) -> str:
    """Export papers as a JSON array."""
    return json.dumps(
        [p.model_dump(mode="json") for p in papers],
        indent=indent,
        ensure_ascii=False,
    )


def export_csv(papers: list[Paper]) -> str:
    """Export papers as CSV."""
    output = io.StringIO()
    writer = csv.writer(output)
    writer.writerow(
        [
            "id",
            "title",
            "authors",
            "year",
            "journal",
            "doi",
            "arxiv_id",
            "pubmed_id",
            "inspire_id",
            "openalex_id",
            "abstract",
            "url",
        ]
    )
    for p in papers:
        writer.writerow(
            [
                p.id,
                p.title,
                "; ".join(p.authors),
                p.year or "",
                p.journal or "",
                p.doi or "",
                p.arxiv_id or "",
                p.pubmed_id or "",
                p.inspire_id or "",
                p.openalex_id or "",
                p.abstract,
                p.url or "",
            ]
        )
    return output.getvalue()


def export_bibtex(papers: list[Paper]) -> str:
    """Export papers as BibTeX entries."""
    entries = []
    for paper in papers:
        key = _generate_bibtex_key(paper)
        entry_type = "article"

        fields = []
        fields.append(f"  title = {{{paper.title}}}")
        if paper.authors:
            fields.append(f"  author = {{{' and '.join(paper.authors)}}}")
        if paper.year:
            fields.append(f"  year = {{{paper.year}}}")
        if paper.journal:
            fields.append(f"  journal = {{{paper.journal}}}")
        if paper.doi:
            fields.append(f"  doi = {{{paper.doi}}}")
        if paper.url:
            fields.append(f"  url = {{{paper.url}}}")
        if paper.arxiv_id:
            fields.append(f"  eprint = {{{paper.arxiv_id}}}")
            fields.append("  archiveprefix = {arXiv}")
        if paper.abstract:
            fields.append(f"  abstract = {{{paper.abstract}}}")

        entry = f"@{entry_type}{{{key},\n" + ",\n".join(fields) + "\n}"
        entries.append(entry)

    return "\n\n".join(entries) + "\n" if entries else ""


def _generate_bibtex_key(paper: Paper) -> str:
    """Generate a BibTeX citation key from paper metadata."""
    # Use first author's last name + year + first word of title
    author_part = ""
    if paper.authors:
        first_author = paper.authors[0]
        # Extract last name (before comma or first word)
        author_part = first_author.split(",")[0].split()[-1].lower()
        author_part = re.sub(r"[^\w]", "", author_part)

    year_part = str(paper.year) if paper.year else ""

    title_words = re.sub(r"[^\w\s]", "", paper.title).split()
    title_part = title_words[0].lower() if title_words else ""

    key = f"{author_part}{year_part}{title_part}"
    return key or paper.id[:8]
