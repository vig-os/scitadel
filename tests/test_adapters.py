"""Tests for source adapter normalization logic.

Uses fixture data to test parsing without real API calls.
"""

from scitadel.adapters.arxiv.adapter import _parse_arxiv_atom
from scitadel.adapters.inspire.adapter import _parse_inspire_results
from scitadel.adapters.openalex.adapter import _reconstruct_abstract, _work_to_candidate
from scitadel.adapters.pubmed.adapter import _parse_pubmed_xml

# -- PubMed fixtures --

PUBMED_XML = """<?xml version="1.0" ?>
<!DOCTYPE PubmedArticleSet PUBLIC "-//NLM//DTD PubMedArticle, 1st January 2024//EN"
  "https://dtd.nlm.nih.gov/ncbi/pubmed/out/pubmed_240101.dtd">
<PubmedArticleSet>
  <PubmedArticle>
    <MedlineCitation>
      <PMID>12345678</PMID>
      <Article>
        <Journal>
          <Title>Nature Methods</Title>
          <JournalIssue>
            <PubDate><Year>2024</Year></PubDate>
          </JournalIssue>
        </Journal>
        <ArticleTitle>PET Tracer Development for Oncology</ArticleTitle>
        <Abstract>
          <AbstractText Label="BACKGROUND">This study examines PET tracers.</AbstractText>
          <AbstractText Label="RESULTS">We found significant results.</AbstractText>
        </Abstract>
        <AuthorList>
          <Author><LastName>Smith</LastName><ForeName>John</ForeName></Author>
          <Author><LastName>Doe</LastName><ForeName>Jane</ForeName></Author>
        </AuthorList>
        <ELocationID EIdType="doi">10.1038/nmeth.2024</ELocationID>
      </Article>
    </MedlineCitation>
  </PubmedArticle>
</PubmedArticleSet>"""


class TestPubMedParser:
    def test_parse_basic_article(self):
        candidates = _parse_pubmed_xml(PUBMED_XML)
        assert len(candidates) == 1
        c = candidates[0]
        assert c.source == "pubmed"
        assert c.source_id == "12345678"
        assert c.pubmed_id == "12345678"
        assert c.title == "PET Tracer Development for Oncology"
        assert c.doi == "10.1038/nmeth.2024"
        assert c.year == 2024
        assert c.journal == "Nature Methods"
        assert len(c.authors) == 2
        assert c.authors[0] == "Smith, John"
        assert "BACKGROUND:" in c.abstract
        assert c.rank == 1

    def test_parse_empty_response(self):
        candidates = _parse_pubmed_xml("<PubmedArticleSet></PubmedArticleSet>")
        assert candidates == []


# -- arXiv fixtures --

ARXIV_ATOM = """<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"
      xmlns:arxiv="http://arxiv.org/schemas/atom">
  <entry>
    <id>http://arxiv.org/abs/2301.12345v1</id>
    <title>  Quantum Computing
  for Drug Discovery  </title>
    <summary>  This paper explores quantum approaches
  to drug discovery.  </summary>
    <author><name>Alice Researcher</name></author>
    <author><name>Bob Scientist</name></author>
    <published>2023-01-15T00:00:00Z</published>
    <arxiv:doi>10.1234/quantum.2023</arxiv:doi>
    <arxiv:journal_ref>Phys. Rev. Lett. 130, 012345 (2023)</arxiv:journal_ref>
    <link title="pdf" href="http://arxiv.org/pdf/2301.12345v1" rel="related"/>
  </entry>
</feed>"""


class TestArxivParser:
    def test_parse_basic_entry(self):
        candidates = _parse_arxiv_atom(ARXIV_ATOM)
        assert len(candidates) == 1
        c = candidates[0]
        assert c.source == "arxiv"
        assert c.arxiv_id == "2301.12345"
        assert c.title == "Quantum Computing for Drug Discovery"
        assert c.doi == "10.1234/quantum.2023"
        assert c.year == 2023
        assert len(c.authors) == 2
        assert "Alice Researcher" in c.authors
        assert "drug discovery" in c.abstract
        assert c.rank == 1

    def test_parse_empty_feed(self):
        empty = '<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"></feed>'
        assert _parse_arxiv_atom(empty) == []


# -- OpenAlex fixtures --


class TestOpenAlexParser:
    def test_reconstruct_abstract(self):
        inverted = {"Hello": [0], "world": [1], "of": [2], "science": [3]}
        assert _reconstruct_abstract(inverted) == "Hello world of science"

    def test_reconstruct_abstract_empty(self):
        assert _reconstruct_abstract({}) == ""

    def test_work_to_candidate(self):
        work = {
            "id": "https://openalex.org/W1234567",
            "title": "Machine Learning in Radiopharma",
            "display_name": "Machine Learning in Radiopharma",
            "publication_year": 2024,
            "doi": "https://doi.org/10.1234/ml-pharma",
            "authorships": [
                {"author": {"display_name": "Dr. Alice"}},
                {"author": {"display_name": "Dr. Bob"}},
            ],
            "abstract_inverted_index": {"ML": [0], "is": [1], "great": [2]},
            "primary_location": {
                "source": {"display_name": "Science"},
            },
            "ids": {"pmid": "https://pubmed.ncbi.nlm.nih.gov/9999"},
        }
        c = _work_to_candidate(work, rank=1)
        assert c.source == "openalex"
        assert c.openalex_id == "W1234567"
        assert c.title == "Machine Learning in Radiopharma"
        assert c.doi == "10.1234/ml-pharma"
        assert c.year == 2024
        assert len(c.authors) == 2
        assert c.pubmed_id == "9999"
        assert c.abstract == "ML is great"
        assert c.journal == "Science"


# -- INSPIRE fixtures --

INSPIRE_RESPONSE = {
    "hits": {
        "hits": [
            {
                "id": 9876543,
                "metadata": {
                    "titles": [{"title": "Detector Physics at the LHC"}],
                    "authors": [
                        {"full_name": "Higgs, Peter"},
                        {"full_name": "Boson, W."},
                    ],
                    "abstracts": [
                        {"value": "We discuss detector upgrades for HL-LHC."}
                    ],
                    "dois": [{"value": "10.1007/lhc-det-2024"}],
                    "arxiv_eprints": [{"value": "2401.99999"}],
                    "publication_info": [
                        {
                            "year": 2024,
                            "journal_title": "JINST",
                        }
                    ],
                },
            }
        ]
    }
}


class TestInspireParser:
    def test_parse_basic_hit(self):
        candidates = _parse_inspire_results(INSPIRE_RESPONSE)
        assert len(candidates) == 1
        c = candidates[0]
        assert c.source == "inspire"
        assert c.inspire_id == "9876543"
        assert c.title == "Detector Physics at the LHC"
        assert c.doi == "10.1007/lhc-det-2024"
        assert c.arxiv_id == "2401.99999"
        assert c.year == 2024
        assert c.journal == "JINST"
        assert len(c.authors) == 2
        assert c.rank == 1

    def test_parse_empty_response(self):
        empty = {"hits": {"hits": []}}
        assert _parse_inspire_results(empty) == []
