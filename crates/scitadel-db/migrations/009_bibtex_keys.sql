-- Stable citation keys for papers (#132 — bib foundation).
--
-- Key is assigned on first encounter (via backfill on 0.6.0 launch or
-- on each new paper thereafter) and never recomputed. Algorithm lives
-- in scitadel-export::bibtex; see ADR-006 for the pinned spec.
--
-- `UNIQUE` because any two papers in the same DB must have distinct
-- citation keys — the disambiguator suffix (`a`/`b`/`c`/...) exists
-- precisely to satisfy this constraint.

ALTER TABLE papers ADD COLUMN bibtex_key TEXT;
CREATE UNIQUE INDEX idx_papers_bibtex_key ON papers(bibtex_key)
    WHERE bibtex_key IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (9, datetime('now'));
