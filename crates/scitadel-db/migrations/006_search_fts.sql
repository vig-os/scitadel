-- Migration 006: FTS5 over search queries for `find_similar_searches`.
--
-- Standalone FTS5 table (not external-content) with an INSERT trigger
-- on `searches` to keep it in sync. Standalone avoids the external-
-- content gotchas around DELETE/UPDATE sync when we only ever insert.
-- Porter stemming + unicode61 tokenizer covers English + diacritics.

CREATE VIRTUAL TABLE IF NOT EXISTS searches_fts USING fts5(
    search_id UNINDEXED,
    query,
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS searches_fts_ai AFTER INSERT ON searches
BEGIN
    INSERT INTO searches_fts(search_id, query) VALUES (new.id, new.query);
END;

-- Backfill any pre-existing rows. Safe on re-run only because migrations
-- are version-gated — this statement fires exactly once when the 006
-- version row is inserted below.
INSERT INTO searches_fts(search_id, query)
    SELECT id, query FROM searches
    WHERE NOT EXISTS (SELECT 1 FROM searches_fts WHERE search_id = searches.id);

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (6, datetime('now'));
