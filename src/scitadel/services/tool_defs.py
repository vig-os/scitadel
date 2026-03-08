"""System prompt and Anthropic tool definitions for the chat engine.

Defines the 11 tools that mirror the MCP server's capabilities,
formatted for the Anthropic Messages API `tools` parameter.
"""

from __future__ import annotations

SYSTEM_PROMPT = """\
You are a scientific literature research assistant with access to federated \
search across PubMed, arXiv, OpenAlex, and INSPIRE-HEP, plus relevance \
scoring and citation chaining tools.

Your workflow:
1. When the user describes a research topic, create a research question with \
create_question.
2. Generate targeted search terms and add them with add_search_terms.
3. Run federated searches using the search tool.
4. Review results and score papers against the question with assess_paper.
5. Use snowball_search to discover related papers via citation chaining.

Be proactive: decompose broad topics into focused search strategies, run \
multiple searches with different term combinations, and score the most \
promising results. Explain your reasoning as you go.

When presenting results, summarize key findings and highlight the most \
relevant papers with their scores."""

TOOLS: list[dict] = [
    {
        "name": "search",
        "description": (
            "Run a federated literature search across scientific databases. "
            "Searches PubMed, arXiv, OpenAlex, and INSPIRE-HEP in parallel. "
            "Results are deduplicated and persisted. "
            "Returns the search ID and summary statistics."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query string",
                },
                "sources": {
                    "type": "string",
                    "description": (
                        "Comma-separated list of sources "
                        "(default: pubmed,arxiv,openalex,inspire)"
                    ),
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results per source (default: 50)",
                },
                "question_id": {
                    "type": "string",
                    "description": (
                        "Research question ID — auto-builds query from "
                        "linked terms if no query provided"
                    ),
                },
            },
        },
    },
    {
        "name": "list_searches",
        "description": "List recent search runs with parameters and results.",
        "input_schema": {
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Max number of searches to return (default: 20)",
                },
            },
        },
    },
    {
        "name": "get_papers",
        "description": (
            "Get all papers from a search, with title, authors, abstract, "
            "DOI, and year. Supports prefix matching on search IDs."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "search_id": {
                    "type": "string",
                    "description": "Search ID (supports prefix matching)",
                },
            },
            "required": ["search_id"],
        },
    },
    {
        "name": "get_paper",
        "description": (
            "Get full details of a single paper by ID (supports prefix matching)."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "paper_id": {
                    "type": "string",
                    "description": "Paper ID (supports prefix matching)",
                },
            },
            "required": ["paper_id"],
        },
    },
    {
        "name": "export_search",
        "description": "Export search results as BibTeX, JSON, or CSV.",
        "input_schema": {
            "type": "object",
            "properties": {
                "search_id": {
                    "type": "string",
                    "description": "Search ID (supports prefix matching)",
                },
                "format": {
                    "type": "string",
                    "enum": ["bibtex", "json", "csv"],
                    "description": "Export format (default: json)",
                },
            },
            "required": ["search_id"],
        },
    },
    {
        "name": "create_question",
        "description": (
            "Create a research question for relevance scoring. Returns the question ID."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The research question text",
                },
                "description": {
                    "type": "string",
                    "description": "Optional context or elaboration",
                },
            },
            "required": ["text"],
        },
    },
    {
        "name": "list_questions",
        "description": "List all research questions.",
        "input_schema": {
            "type": "object",
            "properties": {},
        },
    },
    {
        "name": "add_search_terms",
        "description": (
            "Add search terms linked to a research question. "
            "Terms are used to auto-build search queries."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "question_id": {
                    "type": "string",
                    "description": "Research question ID (supports prefix)",
                },
                "terms": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of keywords",
                },
                "query_string": {
                    "type": "string",
                    "description": "Optional pre-built query string",
                },
            },
            "required": ["question_id", "terms"],
        },
    },
    {
        "name": "assess_paper",
        "description": (
            "Record a relevance assessment for a paper against a research "
            "question. Call after reading a paper's abstract to score "
            "relevance from 0.0 (irrelevant) to 1.0 (highly relevant)."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "paper_id": {
                    "type": "string",
                    "description": "Paper ID (supports prefix)",
                },
                "question_id": {
                    "type": "string",
                    "description": "Research question ID (supports prefix)",
                },
                "score": {
                    "type": "number",
                    "description": "Relevance score 0.0-1.0",
                },
                "reasoning": {
                    "type": "string",
                    "description": "Why this score was assigned",
                },
                "assessor": {
                    "type": "string",
                    "description": "Who made the assessment (default: claude)",
                },
                "model": {
                    "type": "string",
                    "description": "Model name if LLM-assessed",
                },
            },
            "required": ["paper_id", "question_id", "score", "reasoning"],
        },
    },
    {
        "name": "get_assessments",
        "description": (
            "Get relevance assessments, optionally filtered by paper and/or question."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "paper_id": {
                    "type": "string",
                    "description": "Filter by paper ID",
                },
                "question_id": {
                    "type": "string",
                    "description": "Filter by question ID",
                },
            },
        },
    },
    {
        "name": "snowball_search",
        "description": (
            "Run citation chaining (snowballing) from a search's papers. "
            "Discovers related papers via forward and backward citations, "
            "gated by relevance scoring."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "search_id": {
                    "type": "string",
                    "description": "Search ID to snowball from (supports prefix)",
                },
                "question_id": {
                    "type": "string",
                    "description": "Research question for relevance gating",
                },
                "depth": {
                    "type": "integer",
                    "description": "Max chaining depth 1-3 (default: 1)",
                },
                "threshold": {
                    "type": "number",
                    "description": ("Min relevance score to expand (default: 0.6)"),
                },
                "direction": {
                    "type": "string",
                    "enum": ["references", "cited_by", "both"],
                    "description": "Citation direction (default: both)",
                },
                "model": {
                    "type": "string",
                    "description": ("Model for scoring (default: claude-sonnet-4-6)"),
                },
            },
            "required": ["search_id", "question_id"],
        },
    },
]
