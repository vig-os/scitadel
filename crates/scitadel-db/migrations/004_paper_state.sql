-- Migration 004: per-reader paper state (star / to-read / read).
--
-- Single row per (paper, reader). Reader defaults to $USER locally; MCP
-- clients pass an explicit identity string. No users table in v1 — we
-- upgrade to stable UUIDs when Dolt sync lands (Phase 5).

CREATE TABLE IF NOT EXISTS paper_state (
    paper_id   TEXT NOT NULL REFERENCES papers(id),
    reader     TEXT NOT NULL,
    starred    INTEGER NOT NULL DEFAULT 0,   -- SQLite bool
    to_read    INTEGER NOT NULL DEFAULT 0,
    read_at    TEXT,                          -- ISO-8601; NULL = unread
    updated_at TEXT NOT NULL,
    PRIMARY KEY (paper_id, reader)
);

CREATE INDEX IF NOT EXISTS idx_paper_state_reader ON paper_state(reader);
CREATE INDEX IF NOT EXISTS idx_paper_state_starred ON paper_state(starred) WHERE starred = 1;
CREATE INDEX IF NOT EXISTS idx_paper_state_to_read ON paper_state(to_read) WHERE to_read = 1;

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (4, datetime('now'));
