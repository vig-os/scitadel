-- Paper-level tags (#162 — keyword-only Zotero imports).
--
-- Records free-form tags attached to a paper. The driving use case
-- is Zotero `.bib` entries that carry `keywords={a,b,c}` *without*
-- a `note=`: the bib-import pipeline rides along keywords as
-- annotation tags only when there's a note to anchor them on, so
-- keyword-only entries used to drop the user's tags on the floor
-- (#134 PR-A surfaced these as `dropped_keywords` under `--verbose`).
--
-- Annotation tags model text-level tagging — they hang off an anchor.
-- Paper tags hang off the paper itself, which is what Zotero users
-- actually mean when they tag a paper without writing a note.
--
-- `source` is free-form text ("bibtex-import", later: "manual",
-- "csl-json-import", …) so we can audit where a tag came from. The
-- shape mirrors `paper_aliases` (#134 / migration 011) — same
-- (paper_id, value) primary key, same `source` + `created_at`
-- columns, same secondary index on the value column for reverse
-- lookups.

CREATE TABLE IF NOT EXISTS paper_tags (
    paper_id   TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    tag        TEXT NOT NULL,
    source     TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (paper_id, tag)
);

CREATE INDEX IF NOT EXISTS idx_paper_tags_tag ON paper_tags(tag);

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (12, datetime('now'));
