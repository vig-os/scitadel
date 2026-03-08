-- Migration 002: Citation chaining (snowball) tables

CREATE TABLE IF NOT EXISTS citations (
    source_paper_id TEXT NOT NULL REFERENCES papers(id),
    target_paper_id TEXT NOT NULL REFERENCES papers(id),
    direction TEXT NOT NULL,  -- 'references' or 'cited_by'
    discovered_by TEXT NOT NULL DEFAULT '',
    depth INTEGER NOT NULL DEFAULT 0,
    snowball_run_id TEXT,
    UNIQUE(source_paper_id, target_paper_id, direction)
);

CREATE INDEX IF NOT EXISTS idx_citations_source ON citations(source_paper_id);
CREATE INDEX IF NOT EXISTS idx_citations_target ON citations(target_paper_id);
CREATE INDEX IF NOT EXISTS idx_citations_run ON citations(snowball_run_id);

CREATE TABLE IF NOT EXISTS snowball_runs (
    id TEXT PRIMARY KEY,
    search_id TEXT REFERENCES searches(id),
    question_id TEXT REFERENCES research_questions(id),
    direction TEXT NOT NULL DEFAULT 'both',
    max_depth INTEGER NOT NULL DEFAULT 1,
    threshold REAL NOT NULL DEFAULT 0.6,
    total_discovered INTEGER NOT NULL DEFAULT 0,
    total_new_papers INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (2, datetime('now'));
