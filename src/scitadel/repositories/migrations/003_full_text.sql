-- Migration 003: Add full_text and summary columns to papers

ALTER TABLE papers ADD COLUMN full_text TEXT;
ALTER TABLE papers ADD COLUMN summary TEXT;

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (3, datetime('now'));
