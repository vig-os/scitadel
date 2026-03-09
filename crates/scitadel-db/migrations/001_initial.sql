-- Migration 001: Initial schema for Phase 1
-- Covers: papers, searches, search_results, research_questions, search_terms, assessments

CREATE TABLE IF NOT EXISTS papers (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    authors TEXT NOT NULL DEFAULT '[]',  -- JSON array
    abstract TEXT NOT NULL DEFAULT '',
    doi TEXT,
    arxiv_id TEXT,
    pubmed_id TEXT,
    inspire_id TEXT,
    openalex_id TEXT,
    year INTEGER,
    journal TEXT,
    url TEXT,
    source_urls TEXT NOT NULL DEFAULT '{}',  -- JSON object
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_papers_doi ON papers(doi) WHERE doi IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_papers_title ON papers(title);

CREATE TABLE IF NOT EXISTS searches (
    id TEXT PRIMARY KEY,
    query TEXT NOT NULL,
    sources TEXT NOT NULL DEFAULT '[]',  -- JSON array
    parameters TEXT NOT NULL DEFAULT '{}',  -- JSON object
    source_outcomes TEXT NOT NULL DEFAULT '[]',  -- JSON array of SourceOutcome
    total_candidates INTEGER NOT NULL DEFAULT 0,
    total_papers INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS search_results (
    search_id TEXT NOT NULL REFERENCES searches(id),
    paper_id TEXT NOT NULL REFERENCES papers(id),
    source TEXT NOT NULL,
    rank INTEGER,
    score REAL,
    raw_metadata TEXT NOT NULL DEFAULT '{}',  -- JSON object
    PRIMARY KEY (search_id, paper_id, source)
);

CREATE TABLE IF NOT EXISTS research_questions (
    id TEXT PRIMARY KEY,
    text TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS search_terms (
    id TEXT PRIMARY KEY,
    question_id TEXT NOT NULL REFERENCES research_questions(id),
    terms TEXT NOT NULL DEFAULT '[]',  -- JSON array
    query_string TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_search_terms_question ON search_terms(question_id);

CREATE TABLE IF NOT EXISTS assessments (
    id TEXT PRIMARY KEY,
    paper_id TEXT NOT NULL REFERENCES papers(id),
    question_id TEXT NOT NULL REFERENCES research_questions(id),
    score REAL NOT NULL,
    reasoning TEXT NOT NULL DEFAULT '',
    model TEXT,
    prompt TEXT,
    temperature REAL,
    assessor TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_assessments_paper ON assessments(paper_id);
CREATE INDEX IF NOT EXISTS idx_assessments_question ON assessments(question_id);
CREATE INDEX IF NOT EXISTS idx_assessments_paper_question ON assessments(paper_id, question_id);

-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (1, datetime('now'));
