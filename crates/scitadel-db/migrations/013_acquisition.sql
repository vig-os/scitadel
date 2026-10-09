-- Migration 013: literature acquisition (ADR-007, §1 "Data model" and
-- §2 "Status vocabulary").
--
-- The shape this replaces: one file per paper, recorded in three columns
-- on `papers` (`local_path`, `download_status`, `last_attempt_at` --
-- migration 007). raid needs many files per work (full text, SI, tables,
-- figures), each with provenance and licence, plus a coverage report and
-- one grouped list of what still needs a human. That is what these tables
-- are for.
--
-- The load-bearing ideas:
--
--   * A work is the existing `papers` row. It owns many `artefacts`, one
--     row per file, each pointing at a content-addressed `blobs` row. The
--     database is the source of truth; `papers/<stem>/manifest.json` is a
--     generated, read-only mirror, never read back as input.
--   * `blobs` are keyed by sha256 and stored at
--     `<root>/blobs/<first 2 hex>/<sha256>.<ext>`, with `rel_path`
--     relative to the library root (the absolute parent of the DB file).
--     Today's absolute `local_path` values break as soon as a library
--     moves.
--   * There is **no 'have' status**. `acquisition_state` is only what we
--     *want* and have not got; "have" is derived by querying `artefacts`
--     (`access_status = 'full_text'` and a version satisfying
--     `wanted_version`). That keeps a held file and a wanted file from
--     drifting apart, which is what made the three legacy columns
--     untrustworthy.
--   * One status vocabulary, enforced here by CHECK constraints, so
--     `coverage`, `action_list` and `auth_pending` cannot drift apart.
--   * `acquisition_leases` claims a work so two processes against one
--     database file (the TUI + MCP two-pane workflow) do not both fetch
--     it. Leases expire, so a crashed process never blocks a work.
--
-- Time. Pacer fields are Unix epoch milliseconds (`*_ms`, INTEGER);
-- everything else is RFC 3339 UTC TEXT, like the rest of the schema.
--
-- Scope note: the `ALTER TABLE papers` statements below add the identity
-- columns `Work` needs. SQLite has no `ADD COLUMN IF NOT EXISTS`, so
-- these two are version-gated rather than re-runnable on their own --
-- exactly like `full_text`/`summary` (003) and `local_path` (007). The
-- transactional runner (#250) is what makes a failure here recoverable.

-- Content-addressed file store. One row per distinct byte sequence; an
-- artefact may be a reference with no blob at all (a figure URL and
-- caption is enough for a digitisation queue).
CREATE TABLE IF NOT EXISTS blobs (
    sha256     TEXT PRIMARY KEY,
    bytes      INTEGER NOT NULL,
    mime       TEXT,
    rel_path   TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- One row per file belonging to a work.
CREATE TABLE IF NOT EXISTS artefacts (
    id               TEXT PRIMARY KEY,
    paper_id         TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL CHECK (kind IN
                       ('fulltext_pdf','fulltext_html','fulltext_xml','si','table','figure')),
    version          TEXT NOT NULL DEFAULT 'unknown'
                       CHECK (version IN ('vor','am','preprint','unknown')),
    locator          TEXT NOT NULL DEFAULT '',   -- '' for full text; SI/table/figure: stable source id or normalised label
    sha256           TEXT REFERENCES blobs(sha256),   -- NULL only for figure references without a file
    format           TEXT,                       -- pdf|html|jats|json|csv|xlsx|zip|png|…
    access_status    TEXT NOT NULL CHECK (access_status IN ('full_text','abstract','paywall','unknown')),
    derived_from     TEXT REFERENCES artefacts(id) ON DELETE CASCADE,
                       -- which full text a derived artefact was extracted from
                       -- (#234's manifest shape; ADR-007 §3's S3 acceptance
                       -- criterion is the slice that writes the rows). Unwritten
                       -- until #254 lands the JATS `table-wrap` extractor, and
                       -- not before: a self-referential FK nobody fills is
                       -- honest, a writer that always wrote NULL is not.
    route            TEXT NOT NULL,              -- RouteId | 'legacy' | 'import_flat' | 'manual' | 'manual_url'
    source_url       TEXT,
    label            TEXT,
    caption          TEXT,
    publisher        TEXT,
    license_url      TEXT,                       -- licence + provenance; derived artefacts copy these from derived_from
    license_content_version TEXT,                -- vor|am|tdm|stm-asf|unspecified (Crossref)
    license_start    TEXT,
    license_source   TEXT,                       -- crossref|openalex|unpaywall|europepmc|pmc|publisher
    access_basis     TEXT NOT NULL CHECK (access_basis IN
                       ('oa_license','tdm_licence','statutory_24d','subscription_read','ill','manual')),
    tdm_policy_ref   TEXT,
    auth_context     TEXT,                       -- 'none' | 'ip' | 'token:<fingerprint>' | 'session:<publisher>'
    retain_until     TEXT,
    imported_from    TEXT,                       -- original path for legacy / flat-layout imports
    retrieved_at     TEXT NOT NULL,
    missing_on_disk  INTEGER NOT NULL DEFAULT 0,
    UNIQUE (paper_id, kind, version, locator)
);
CREATE INDEX IF NOT EXISTS idx_artefacts_paper   ON artefacts(paper_id);
CREATE INDEX IF NOT EXISTS idx_artefacts_derived ON artefacts(derived_from);
CREATE INDEX IF NOT EXISTS idx_artefacts_sha     ON artefacts(sha256);

-- Serves the legacy backfill's scan (`WHERE local_path IS NOT NULL AND
-- local_path <> ''`). That backfill runs on every `migrate()`, so
-- without this the common case -- a library where every legacy file is
-- already recorded -- would still table-scan every paper on every
-- startup. Partial, because a paper with no legacy path is the
-- overwhelming majority and storing those keys buys nothing.
CREATE INDEX IF NOT EXISTS idx_papers_local_path ON papers(local_path)
    WHERE local_path IS NOT NULL AND local_path <> '';

-- What we want and haven't got. There is no 'have' status: "have" is derived (see above).
CREATE TABLE IF NOT EXISTS acquisition_state (
    paper_id        TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN
                      ('fulltext','fulltext_pdf','fulltext_html','fulltext_xml','si','table','figure')),
    locator         TEXT NOT NULL DEFAULT '',
    wanted_version  TEXT NOT NULL DEFAULT 'vor' CHECK (wanted_version IN ('vor','am','preprint','any')),
    status          TEXT NOT NULL CHECK (status IN
                      ('pending','oa_fetchable','tdm_available','tdm_key_missing','needs_login',
                       'not_entitled','needs_authorisation','needs_ill','unavailable',
                       'identity_mismatch','wrong_version','rate_limited','error')),
    reason          TEXT,
    publisher       TEXT,
    hint_url        TEXT,
    drop_path       TEXT,
    next_attempt_at TEXT,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (paper_id, kind, locator)
);

-- Audit trail: survives paper deletion (paper_id set to NULL).
CREATE TABLE IF NOT EXISTS acquisition_attempts (
    id          INTEGER PRIMARY KEY,
    paper_id    TEXT REFERENCES papers(id) ON DELETE SET NULL,
    kind        TEXT NOT NULL,
    locator     TEXT NOT NULL DEFAULT '',
    route       TEXT NOT NULL,
    bucket      TEXT,
    started_at  TEXT NOT NULL,
    outcome     TEXT NOT NULL,                  -- ok|http_error|login_redirect|too_large|bad_magic|identity_mismatch|denied|cancelled
    http_status INTEGER,
    detail      TEXT
);
CREATE INDEX IF NOT EXISTS idx_attempts_paper ON acquisition_attempts(paper_id);

-- A work is claimed by the upsert below; no row returned means another
-- live process holds it. The holder renews while fetching, and an expired
-- lease can be taken over, so a crash never blocks a work for good.
CREATE TABLE IF NOT EXISTS acquisition_leases (
    paper_id       TEXT PRIMARY KEY REFERENCES papers(id) ON DELETE CASCADE,
    owner          TEXT NOT NULL,               -- pid + random nonce
    lease_until_ms INTEGER NOT NULL
);

-- Identity is checked twice (ADR-007 §3): pre-fetch against the resolved
-- title, post-fetch against the served page or PDF. A mismatch blocks
-- filing anything under that DOI.
CREATE TABLE IF NOT EXISTS paper_identity_checks (
    id              INTEGER PRIMARY KEY,
    paper_id        TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    checked_at      TEXT NOT NULL,
    phase           TEXT NOT NULL CHECK (phase IN ('pre_fetch','post_fetch')),
    source          TEXT NOT NULL,              -- openalex|crossref|datacite|served_page
    expected_title  TEXT,
    resolved_title  TEXT,
    score           REAL,
    status          TEXT NOT NULL CHECK (status IN ('ok','mismatch','unverified','overridden')),
    override_reason TEXT,
    doi_corrected_from TEXT
);
CREATE INDEX IF NOT EXISTS idx_identity_paper ON paper_identity_checks(paper_id);

-- The persistent, per-publisher-platform ledger (ADR-007 §4). Buckets are
-- publisher platforms, not hosts; an unlisted host gets a bucket named
-- after its registrable domain. `next_allowed_ms` paces a bucket's
-- minimum interval, `backoff_until_ms` parks it after a login redirect or
-- a 429 -- both epoch milliseconds, because waits are computed as
-- durations and slept with tokio's Instant, never by subtracting
-- wall-clock values in a loop.
CREATE TABLE IF NOT EXISTS pacer_buckets (
    bucket           TEXT PRIMARY KEY,
    next_allowed_ms  INTEGER NOT NULL DEFAULT 0,
    backoff_until_ms INTEGER NOT NULL DEFAULT 0,
    backoff_reason   TEXT
);

-- One row per permit taken. A permit counts when it is granted, not when
-- the fetch succeeds, and grants older than 24 h are pruned.
CREATE TABLE IF NOT EXISTS pacer_grants (
    bucket         TEXT NOT NULL,
    tier           TEXT NOT NULL CHECK (tier IN ('meta','oa','tdm','session')),
    unit           TEXT NOT NULL CHECK (unit IN ('request','work')),
    granted_at_ms  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pacer_grants_bucket ON pacer_grants(bucket, granted_at_ms);
CREATE INDEX IF NOT EXISTS idx_pacer_grants_tier   ON pacer_grants(tier, unit, granted_at_ms);

-- Per-publisher authorisation for tier 2 and, gated on
-- `allows_bulk_session`, tier 4 bulk draining. No secrets here. A row may
-- only *tighten* a cap below the §4 defaults, never raise it.
CREATE TABLE IF NOT EXISTS tdm_authorisations (           -- no secrets here
    publisher           TEXT PRIMARY KEY,
    basis               TEXT NOT NULL CHECK (basis IN ('api_terms','library_agreement','platform_policy')),
    policy_ref          TEXT NOT NULL,
    allows_bulk_session INTEGER NOT NULL DEFAULT 0,
    max_works_per_day   INTEGER,              -- never raises a cap above the §4 defaults; can only tighten
    granted_by          TEXT NOT NULL,
    recorded_at         TEXT NOT NULL,
    expires_at          TEXT
);

-- Identity columns on the work itself. `pmcid` unlocks the PMC OA dataset
-- and Europe PMC full text; `osti_id` covers DOE national-lab reports
-- that have no DOI. The partial unique indexes are what make these usable
-- as deduplication keys -- rows without a value are exempt, so papers
-- that lack an identifier are unaffected.
ALTER TABLE papers ADD COLUMN pmcid TEXT;
ALTER TABLE papers ADD COLUMN osti_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_papers_pmcid ON papers(pmcid)   WHERE pmcid   IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_papers_osti  ON papers(osti_id) WHERE osti_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (13, datetime('now'));
