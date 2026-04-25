-- Paper citation aliases (#134 — bib import iter 2).
--
-- Records alternative citation keys a paper is known by — primarily
-- the citekey from an imported `.bib` file. The paper's authoritative
-- key lives on `papers.bibtex_key` (#132) and is never rewritten on
-- import; the imported key is kept here as an alias so `verify` /
-- lookup code can still resolve `smith2024old` → paper-id after
-- scitadel has (re)assigned `smith2024machine`.
--
-- `source` is free-form text ("bibtex-import", later: "manual",
-- "csl-json-import", …) so we can audit where an alias came from.
--
-- No UNIQUE on `alias` alone — two papers in two different imported
-- .bib files may legitimately collide on a citekey. Disambiguation is
-- on the scitadel side via `papers.bibtex_key`.

CREATE TABLE IF NOT EXISTS paper_aliases (
    paper_id   TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    alias      TEXT NOT NULL,
    source     TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (paper_id, alias)
);

CREATE INDEX IF NOT EXISTS idx_paper_aliases_alias ON paper_aliases(alias);

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (11, datetime('now'));
