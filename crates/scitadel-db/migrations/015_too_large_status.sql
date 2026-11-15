-- Migration 015: extend `acquisition_state.status` with `too_large`
-- (#288, ADR-007 §1 "Storage", §2 "Status vocabulary").
--
-- Pre-#288 the fetch cap did not exist, so a cap-refused fetch did not
-- either. #288 added the cap and recorded the refusal as `unavailable` —
-- the same status ADR-007 §2 gives to "the work has no such artefact" —
-- so `action_list` lumped a work whose body was over the per-kind cap
-- into the deferred "no SI" bucket. The two are materially different:
-- the first is *our* refusal and a user can act on it (raise the cap or
-- fetch manually and import), the second is a property of the work and
-- there is nothing to do. `action_list` groups by `status`, so the
-- distinction must live in the column.
--
-- `too_large` is a terminal "no": a publisher that served an oversized
-- body today will serve the same oversized body tomorrow, so re-asking
-- on every `acquire` run is wrong. It joins `unavailable` outside
-- `acquire::ACQUIRE_QUEUE_STATUSES`; it is the one terminal status the
-- queue *can* cancel against a raised cap, through a named re-acquire
-- (ADR-007 §2's "retry unchanged achieves nothing" is why the queue
-- filter excludes it; an explicit `--paper <id>` bypasses the filter
-- because the caller is declaring they changed the inputs).
--
-- Scope note: SQLite has no `ALTER CHECK`, so the table is rebuilt —
-- same pattern migration 006 uses to rename `search_fts`. No table
-- references `acquisition_state`, so FK handling is a non-concern; the
-- `papers` FK is restated on the new table so `ON DELETE CASCADE`
-- survives the rebuild.

CREATE TABLE acquisition_state_new (
    paper_id        TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN
                      ('fulltext','fulltext_pdf','fulltext_html','fulltext_xml','si','table','figure')),
    locator         TEXT NOT NULL DEFAULT '',
    wanted_version  TEXT NOT NULL DEFAULT 'vor' CHECK (wanted_version IN ('vor','am','preprint','any')),
    status          TEXT NOT NULL CHECK (status IN
                      ('pending','oa_fetchable','tdm_available','tdm_key_missing','needs_login',
                       'not_entitled','needs_authorisation','needs_ill','unavailable',
                       'identity_mismatch','wrong_version','rate_limited','too_large','error')),
    reason          TEXT,
    publisher       TEXT,
    hint_url        TEXT,
    drop_path       TEXT,
    next_attempt_at TEXT,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (paper_id, kind, locator)
);

INSERT INTO acquisition_state_new
    (paper_id, kind, locator, wanted_version, status, reason, publisher,
     hint_url, drop_path, next_attempt_at, updated_at)
SELECT paper_id, kind, locator, wanted_version, status, reason, publisher,
       hint_url, drop_path, next_attempt_at, updated_at
FROM acquisition_state;

DROP TABLE acquisition_state;
ALTER TABLE acquisition_state_new RENAME TO acquisition_state;

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (15, datetime('now'));
