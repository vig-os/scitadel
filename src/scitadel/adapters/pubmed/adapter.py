"""PubMed source adapter using E-utilities API.

Docs: https://www.ncbi.nlm.nih.gov/books/NBK25497/
Rate limit: 3 req/s without API key, 10 req/s with.
"""

from __future__ import annotations

import defusedxml.ElementTree as ET
import httpx

from scitadel.domain.models import CandidatePaper

ESEARCH_URL = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi"
EFETCH_URL = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi"


class PubMedAdapter:
    """PubMed E-utilities adapter."""

    def __init__(self, api_key: str = "", timeout: float = 30.0) -> None:
        self._api_key = api_key
        self._timeout = timeout

    @property
    def name(self) -> str:
        return "pubmed"

    async def search(
        self,
        query: str,
        max_results: int = 50,
        **params: object,
    ) -> list[CandidatePaper]:
        """Search PubMed and return normalized candidate records."""
        async with httpx.AsyncClient(timeout=self._timeout) as client:
            pmids = await self._esearch(client, query, max_results)
            if not pmids:
                return []
            return await self._efetch(client, pmids)

    async def _esearch(
        self, client: httpx.AsyncClient, query: str, max_results: int
    ) -> list[str]:
        """Search for PMIDs matching the query."""
        search_params: dict[str, str | int] = {
            "db": "pubmed",
            "term": query,
            "retmax": max_results,
            "retmode": "json",
            "sort": "relevance",
        }
        if self._api_key:
            search_params["api_key"] = self._api_key

        resp = await client.get(ESEARCH_URL, params=search_params)
        resp.raise_for_status()
        data = resp.json()
        return data.get("esearchresult", {}).get("idlist", [])

    async def _efetch(
        self, client: httpx.AsyncClient, pmids: list[str]
    ) -> list[CandidatePaper]:
        """Fetch article details for a list of PMIDs."""
        fetch_params: dict[str, str] = {
            "db": "pubmed",
            "id": ",".join(pmids),
            "retmode": "xml",
        }
        if self._api_key:
            fetch_params["api_key"] = self._api_key

        resp = await client.get(EFETCH_URL, params=fetch_params)
        resp.raise_for_status()
        return _parse_pubmed_xml(resp.text)


def _parse_pubmed_xml(xml_text: str) -> list[CandidatePaper]:
    """Parse PubMed XML response into CandidatePaper records."""
    root = ET.fromstring(xml_text)
    candidates = []

    for rank, article in enumerate(root.findall(".//PubmedArticle"), start=1):
        medline = article.find("MedlineCitation")
        if medline is None:
            continue

        pmid_el = medline.find("PMID")
        pmid = pmid_el.text if pmid_el is not None else ""

        art = medline.find("Article")
        if art is None:
            continue

        title_el = art.find("ArticleTitle")
        title = title_el.text or "" if title_el is not None else ""

        # Authors
        authors = []
        author_list = art.find("AuthorList")
        if author_list is not None:
            for author in author_list.findall("Author"):
                last = author.findtext("LastName", "")
                first = author.findtext("ForeName", "")
                if last:
                    authors.append(f"{last}, {first}".strip(", "))

        # Abstract
        abstract_el = art.find("Abstract")
        abstract = ""
        if abstract_el is not None:
            parts = []
            for text_el in abstract_el.findall("AbstractText"):
                label = text_el.get("Label", "")
                text = text_el.text or ""
                if label:
                    parts.append(f"{label}: {text}")
                else:
                    parts.append(text)
            abstract = " ".join(parts)

        # DOI
        doi = None
        for eid in art.findall(".//ELocationID"):
            if eid.get("EIdType") == "doi":
                doi = eid.text

        # Also check ArticleIdList in PubmedData
        pubmed_data = article.find("PubmedData")
        if pubmed_data is not None and doi is None:
            for aid in pubmed_data.findall(".//ArticleId"):
                if aid.get("IdType") == "doi":
                    doi = aid.text

        # Journal
        journal_el = art.find("Journal/Title")
        journal = journal_el.text if journal_el is not None else None

        # Year
        year = None
        pub_date = art.find("Journal/JournalIssue/PubDate")
        if pub_date is not None:
            year_el = pub_date.find("Year")
            if year_el is not None and year_el.text:
                try:
                    year = int(year_el.text)
                except ValueError:
                    pass

        candidates.append(
            CandidatePaper(
                source="pubmed",
                source_id=pmid,
                title=title,
                authors=authors,
                abstract=abstract,
                doi=doi,
                pubmed_id=pmid,
                year=year,
                journal=journal,
                url=f"https://pubmed.ncbi.nlm.nih.gov/{pmid}/",
                rank=rank,
            )
        )

    return candidates
