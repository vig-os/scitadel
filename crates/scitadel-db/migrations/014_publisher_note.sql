-- Migration 014: `artefacts.publisher_note` (#262, ADR-007 §1 "Legacy data"
-- / §3 "Routes and the ladder").
--
-- `artefacts.publisher` (migration 013) can only ever hold a name we
-- actually know, and the prefix -> publisher table is deliberately
-- incomplete -- it is allowed to be incomplete, because its
-- incompleteness is visible. What the column cannot express on its own is
-- *why* it is empty, and the why matters because two very different
-- statements share one empty cell:
--
--   "we classified the publisher and it has no TDM route"  -- an
--       established negative, and the only one that may suggest a route
--       is not worth looking for;
--   "we never classified the publisher, so no route was evaluated" --
--       a gap in our own coverage, which is what 249 papers were told
--       during the #261 audit.
--
-- Storing that distinction per artefact is the point of this column, so
-- it is written verbatim from
-- `RouteVerdict::PublisherUnknown { prefix }.note()` and never hand-typed
-- into a looser phrasing.
--
-- NULL for a row whose publisher *was* classified: the name is the whole
-- record there, and a note would assert a TDM route had been evaluated
-- for a download that never evaluated one (S1 wires no TDM route).
--
-- Scope note: SQLite has no `ADD COLUMN IF NOT EXISTS`, so this is
-- version-gated through `schema_version` like `full_text`/`summary` (003)
-- and `local_path` (007).

ALTER TABLE artefacts ADD COLUMN publisher_note TEXT;

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (14, datetime('now'));