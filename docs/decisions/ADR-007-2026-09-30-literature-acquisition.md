# ADR-007: Literature acquisition (works, artefacts, route ladder, pacer, access boundary)

**Status**: Accepted
**Date**: 2026-09-30
**Supersedes**: —
**Context**: #247 (spike), #230 (paywall access ladder), #234 (corpus acquisition)

## Context

scitadel downloads one file per paper today. `download_paper`
(`crates/scitadel-adapters/src/download.rs`) walks arXiv → OpenAlex →
Unpaywall → publisher HTML → URL and records the outcome in three columns
on `papers` (`local_path`, `download_status`, `last_attempt_at`; migration
007). Its first consumer, raid (a radiopharmaceutical dataset project),
needs more than that for a curated corpus:

- full text, supplementary information (SI), tables and figures for each
  work;
- provenance and licence for each file;
- a coverage report, and one grouped list of what still needs a human;
- a way past paywalls that doesn't get the institution's access suspended.

As of 2026-09-29, 212 of raid's 587 DOIs are paywalled and no SI is on disk.

#230 and #234 describe two halves of one system. This ADR fixes the
parts every later slice depends on, so each slice can ship on its own
against a stable contract. It reflects five independent reviews (data
architecture, research-library licensing, security, Rust async, and the
raid consumer; consolidated on #247) and the maintainer's decisions on
the points where the reviews disagreed.

## Decision

1. **Data model.** A work is the existing `papers` row. It owns many
   **artefacts**: one row per file, pointing at a content-addressed
   **blob**. The database is the source of truth.
   `papers/<stem>/manifest.json` is a generated, read-only mirror.
2. **One status vocabulary.** `acquisition_state`, keyed by
   `(work, kind, locator)`, backs coverage, `action_list` and
   `auth_pending`. None of the three has its own store.
3. **Resolve, then rank.** For each work, a metadata pass (which touches
   no publisher hosts) finds every candidate location. The ladder then
   picks by **version and licence**, not by which route answered first.
4. **One paced client.** Every outbound request, redirect hops included,
   goes through `PacedClient`. It takes a permit from a **persistent,
   per-publisher-platform** ledger in SQLite. Routes cannot get to a raw
   HTTP client.
5. **Access tiers.**
   - Open access comes first.
   - Tier 2 is **publisher-sanctioned TDM only** (API or platform, under
     the publisher's own terms).
   - There is **no generic institutional-IP tier**.
   - Tier 4 runs in a **dedicated browser profile** that scitadel
     launches. It ships as **one-click "save what I opened"**. Bulk
     draining is gated on a per-publisher `tdm_authorisation` record.
6. **The agent boundary.** Agents may queue works and read status and
   artefacts. Agents never supply URLs, paths, browser options or pacing,
   and they never trigger browser fetching.

## 1 · Data model (migration 013)

The paths below are **relative to the library root**, which is the
directory holding the DB. Today's absolute `local_path` values stop
working as soon as a library moves.

```sql
-- Bytes, deduplicated by content.
CREATE TABLE blobs (
    sha256     TEXT PRIMARY KEY,
    bytes      INTEGER NOT NULL,
    mime       TEXT,
    rel_path   TEXT NOT NULL,          -- blobs/<aa>/<sha256>.<ext>
    created_at TEXT NOT NULL
);

-- One row per file we hold for a work.
CREATE TABLE artefacts (
    id               TEXT PRIMARY KEY,
    paper_id         TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL CHECK (kind IN
                       ('fulltext_pdf','fulltext_html','fulltext_xml','si','table','figure')),
    version          TEXT NOT NULL DEFAULT 'unknown'
                       CHECK (version IN ('vor','am','preprint','unknown')),
    locator          TEXT NOT NULL DEFAULT '',   -- '' for full text; SI/table/figure: stable source id or normalised label
    sha256           TEXT REFERENCES blobs(sha256),   -- NULL for figure references without a file
    format           TEXT,                       -- pdf|html|jats|json|csv|xlsx|zip|png|…
    access_status    TEXT NOT NULL CHECK (access_status IN ('full_text','abstract','paywall','unknown')),
    derived_from     TEXT REFERENCES artefacts(id) ON DELETE CASCADE,
    route            TEXT NOT NULL,              -- RouteId, 'legacy', 'import_flat', 'manual', 'manual_url'
    source_url       TEXT,
    label            TEXT,                       -- "Table 2", "Supporting Information S1"
    caption          TEXT,
    publisher        TEXT,
    -- licence and provenance (derived artefacts copy these from derived_from)
    license_url      TEXT,
    license_content_version TEXT,                -- vor|am|tdm|stm-asf|unspecified
    license_start    TEXT,
    license_source   TEXT,                       -- crossref|openalex|unpaywall|europepmc|pmc|publisher
    access_basis     TEXT NOT NULL CHECK (access_basis IN
                       ('oa_license','tdm_licence','statutory_24d','subscription_read','ill','manual')),
    tdm_policy_ref   TEXT,
    auth_context     TEXT,                       -- 'none' | 'ip' | 'token:<fingerprint>' | 'session:<publisher>'
    retain_until     TEXT,                       -- e.g. Springer: storage limited to the TDM project
    imported_from    TEXT,                       -- original path for legacy / flat-layout imports
    retrieved_at     TEXT NOT NULL,
    missing_on_disk  INTEGER NOT NULL DEFAULT 0,
    UNIQUE (paper_id, kind, version, locator)
);
CREATE INDEX idx_artefacts_paper ON artefacts(paper_id);

-- What we want and haven't got. There is no 'have' row: "have" is derived from artefacts.
CREATE TABLE acquisition_state (
    paper_id        TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,
    locator         TEXT NOT NULL DEFAULT '',    -- '' = "want this kind"; else a known-but-unfetched SI file
    wanted_version  TEXT NOT NULL DEFAULT 'vor',
    status          TEXT NOT NULL,               -- see §2
    reason          TEXT,
    publisher       TEXT,
    hint_url        TEXT,
    drop_path       TEXT,                        -- where a human-supplied file will be picked up
    next_attempt_at TEXT,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (paper_id, kind, locator)
);

CREATE TABLE acquisition_attempts (          -- append-only audit trail
    id          INTEGER PRIMARY KEY,
    paper_id    TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    route       TEXT NOT NULL,
    bucket      TEXT,
    started_at  TEXT NOT NULL,
    outcome     TEXT NOT NULL,
    http_status INTEGER,
    detail      TEXT
);

-- Cross-process work leases: one fetcher per work at a time.
CREATE TABLE acquisition_leases (
    paper_id       TEXT PRIMARY KEY REFERENCES papers(id) ON DELETE CASCADE,
    owner          TEXT NOT NULL,                -- pid + random nonce
    lease_until_ms INTEGER NOT NULL
);

-- Identity checks with history; manual overrides are recorded, never silent.
CREATE TABLE paper_identity_checks (
    id              INTEGER PRIMARY KEY,
    paper_id        TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    checked_at      TEXT NOT NULL,
    phase           TEXT NOT NULL CHECK (phase IN ('pre_fetch','post_fetch')),
    source          TEXT NOT NULL,               -- openalex|crossref|datacite|served_page
    expected_title  TEXT,
    resolved_title  TEXT,
    score           REAL,
    status          TEXT NOT NULL CHECK (status IN ('ok','mismatch','unverified','overridden')),
    override_reason TEXT,
    doi_corrected_from TEXT
);

-- Pacer (§4).
CREATE TABLE pacer_buckets (
    bucket           TEXT PRIMARY KEY,            -- publisher platform, e.g. 'elsevier', 'europepmc'
    next_allowed_ms  INTEGER NOT NULL DEFAULT 0,
    backoff_until_ms INTEGER NOT NULL DEFAULT 0,
    backoff_reason   TEXT
);
CREATE TABLE pacer_grants (
    bucket         TEXT NOT NULL,
    granted_at_ms  INTEGER NOT NULL,
    unit           TEXT NOT NULL CHECK (unit IN ('request','work'))
);
CREATE INDEX idx_pacer_grants ON pacer_grants(bucket, granted_at_ms);

-- Tier-2 / tier-4 authorisation per publisher (§5). No secrets in here.
CREATE TABLE tdm_authorisations (
    publisher      TEXT PRIMARY KEY,
    basis          TEXT NOT NULL,                 -- 'api_terms' | 'library_agreement' | 'platform_policy'
    policy_ref     TEXT NOT NULL,                 -- URL or agreement id
    allows_bulk_session INTEGER NOT NULL DEFAULT 0,
    max_works_per_day   INTEGER,
    granted_by     TEXT NOT NULL,                 -- the human who recorded it
    recorded_at    TEXT NOT NULL,
    expires_at     TEXT
);

ALTER TABLE papers ADD COLUMN pmcid TEXT;
ALTER TABLE papers ADD COLUMN osti_id TEXT;
CREATE UNIQUE INDEX idx_papers_pmcid ON papers(pmcid) WHERE pmcid IS NOT NULL;
CREATE UNIQUE INDEX idx_papers_osti  ON papers(osti_id) WHERE osti_id IS NOT NULL;
```

**Tables** are stored as structured JSON (`format = 'json'`: caption,
label, anchor, footnotes, and cells with `rowspan` / `colspan` / `th`)
derived from JATS or HTML. CSV is an export view only, because CSV
cannot represent merged cells.

**Figures** may be *references*: an artefact with `sha256 IS NULL`,
`source_url` and `caption`. raid's digitisation queue needs the URL and
caption without the image being downloaded.

**SI** is `kind = 'si'`, with `locator` set to the normalised source
filename or ID. A file only counts as SI if its magic bytes match its
declared format: raid has seven "SI" files that are really HTML landing
pages.

**The manifest mirror** is written only by the process holding the
work's lease, after the DB commit, via a temporary file and a rename. It
is never read as input.

**Manual drop-ins** are reconciled by an explicit `scitadel scan` or
`scitadel attach <paper> <file> --kind …`, never as a side effect of a
read. Unknown files become `route = 'manual'`,
`access_status = 'unknown'`, and get a post-fetch identity check. A
recorded file that has vanished is flagged `missing_on_disk = 1`; the row
is kept.

### Legacy data

This runs in Rust after the SQL of migration 013, is idempotent
(`INSERT OR IGNORE` on the UNIQUE key), and happens inside a transactional
migration runner (#250). The TUI has always folded
Abstract / Paywall / Unknown into `paywall`, so no real access status
survives in the old data. The mapping is therefore conservative:

| legacy `download_status` | result |
|---|---|
| `downloaded`, file exists | artefact, `access_status = 'full_text'`, `route = 'legacy'`, `access_basis = 'manual'` |
| `paywall`, file exists | artefact, `access_status = 'unknown'`; full-text state stays `pending` |
| `failed` | `acquisition_state.status = 'error'`, carrying over `last_attempt_at` |
| file no longer exists | no artefact |

A **flat-layout importer** (`scitadel import-flat <dir>`) brings in
consumer-written files: raid's
`<slug>.{pdf,html,tables.json,meta.json}` and `<slug>_SI_<name>`. It
records `imported_from`, because a published datasheet cites those paths
as provenance.

The legacy columns (`local_path`, `download_status`, `last_attempt_at`)
**stop being written** in the release that ships migration 013, so they
can't drift from the artefacts. They are dropped one release later.

## 2 · Status vocabulary (`acquisition_state.status`)

| status | meaning | action_list group |
|---|---|---|
| `pending` | never tried | — (picked up by `acquire`) |
| `oa_fetchable` | an OA location is known, not yet fetched | — |
| `tdm_available` | a sanctioned TDM route exists and credentials are present | — |
| `tdm_key_missing` | a sanctioned TDM route exists, no credential | "register key for <publisher>" |
| `needs_login` | tier 4 is possible, no live session | "log in to <publisher> in the scitadel browser" |
| `not_entitled` | authenticated, still 403 | "ILL / library request" (not "log in again") |
| `needs_authorisation` | only bulk tier 4 could fetch it, and no `tdm_authorisation` exists | "open & save (one click each) or ask library" |
| `needs_ill` | no route exists (includes works without a DOI) | "ILL request" with exported citation |
| `unavailable` | the work has no such artefact (e.g. no SI) | — |
| `identity_mismatch` | the resolved or served title doesn't match the expected one | "check DOI" with both titles |
| `wrong_version` | only a non-wanted version is held (e.g. AM while VoR is wanted) | grouped by the route that could supply the wanted version |
| `rate_limited` | the bucket is in backoff or at its cap | — (retry after `next_attempt_at`) |
| `error` | transient failure | — (retry with backoff) |

"Have" is derived. Coverage is computed from `artefacts`, meeting the
wanted version, plus `acquisition_state`. `action_list` groups rows by
`(status group, publisher)` and carries `drop_path` and `hint_url`. Its
counts sum exactly to the missing totals in `coverage`.

## 3 · Routes and the ladder

```rust
#[async_trait]
pub trait Route: Send + Sync {
    fn id(&self) -> RouteId;
    fn tier(&self) -> Tier;                       // Oa | Tdm | Session
    fn likely_buckets(&self, w: &Work) -> Vec<Bucket>;  // for --dry-run coverage only
    fn applies(&self, w: &Work, want: &KindSet) -> bool;
    async fn fetch(&self, cx: &RouteCtx, w: &Work, want: &KindSet) -> RouteOutcome;
}

pub struct RouteOutcome {
    pub got: Vec<NewArtefact>,
    pub status: BTreeMap<Kind, AcqStatus>,        // got ∪ status covers `want` (asserted by the ladder)
}
```

- `#[async_trait]`, because the ladder is a `Vec<Arc<dyn Route>>` and
  `SourceAdapter` already works this way.
- Routes must be cancel-safe: download to a temp file, check the sha256
  and the magic bytes, then persist. `RouteCtx` carries a
  `CancellationToken`.

**Resolve, then rank.** For each work, one metadata pass collects
candidate locations from OpenAlex (all `locations[]`), Unpaywall,
Crossref (`license[]`, `link[intended-application=text-mining]`,
relations), DataCite and Europe PMC (`isOpenAccess`, `inEPMC`). None of
these touch publisher hosts. Candidates are ranked **OA VoR > AM >
preprint**, then by licence strength. Crossref `license[]` counts as OA
only when it is an allow-listed CC URL that is already in force
(`content-version`, `start`, `delay-in-days`); ACS's own policy URLs
don't count.

**Fetch order** within the ranked candidates:

1. **Europe PMC** `fullTextXML` (JATS) and `supplementaryFiles`, only
   when `isOpenAccess` is set or it's an author manuscript (`version = am`,
   `license = NULL`). `inEPMC = Y` alone means free to read, not a reuse
   licence.
2. **PMC OA dataset** (`pmc-oa-opendata` S3, anonymous HTTPS; the
   `oa.fcgi` service was retired on 2026-08-25), using its per-version
   JSON for licence and `is_manuscript`.
3. **OpenAlex / Unpaywall OA locations** (PDF and landing pages of
   *repositories*), arXiv for preprints, and **DOE OSTI** for
   national-lab reports.
4. **SI metadata:** DataCite `IsSupplementTo` (Taylor & Francis on
   Figshare, Zenodo, Dryad), plus a cheap Crossref `is-supplemented-by`
   check (≈0 for ACS/RSC). SI files hosted by publishers (ACS, RSC) are
   fetched through the **publisher's** bucket.
5. **Tier 2, sanctioned TDM:**
   - **Elsevier:** Article Retrieval and Object APIs (full text and `mmc`
     SI); API key, plus `insttoken` off campus; non-commercial.
   - **Wiley:** TDM token.
   - **Springer Nature:** platform download at ≤1 req/s. The Springer
     API key only unlocks the OA API unless a TDM API subscription
     exists.
   - Publishers with no self-serve API (ACS, RSC, T&F, SNMMI) need a
     `tdm_authorisation` negotiated by the library before any tier-2 use.
   - **ScienceDirect is never scraped.**
6. **Landing-page HTML** for OA works only, after TDM, never for
   ScienceDirect.
7. **Tier 4, browser session** (§5).
8. Otherwise `needs_ill`, `needs_authorisation` or `not_entitled`, with a
   `drop_path`.

**Identity** is checked twice. Before fetching, the expected title is
compared with the resolved one (OpenAlex → Crossref → DataCite, fuzzy
match, year ±1, first author as tiebreak). After fetching, the served
page or PDF title is compared with OpenAlex, which catches redirects to
the wrong paper. A mismatch blocks filing anything under that DOI.
`doi_corrected_from` is recorded when a DOI is fixed.

## 4 · Pacer

**Unbypassable by construction.**

- Routes live in a crate, `scitadel-acquire`, that does not depend on
  `reqwest`. It gets `PacedClient`, `Url`, `StatusCode` and
  `PacedResponse` only.
- The single `reqwest::Client` inside `PacedClient` is built with
  `redirect(Policy::none())`. `PacedClient` follows redirects itself and
  takes **one permit per hop's bucket**. A redirect to a login page
  becomes `needs_login` and triggers a backoff.
- A clippy lint (`disallowed-types = ["reqwest::Client"]`,
  `disallowed-methods = ["reqwest::get", …]`) is allowed only in
  `paced.rs`.
- The metadata adapters (OpenAlex, Crossref, DataCite, Unpaywall,
  Europe PMC) move onto `PacedClient` too.

**Buckets are publisher platforms, not hosts.** A policy table maps
hosts to buckets; for example, `sciencedirect.com`,
`pdf.sciencedirectassets.com`, `ars.els-cdn.com`,
`linkinghub.elsevier.com` and `api.elsevier.com` are all `elsevier`.
Unknown hosts get a conservative default bucket keyed by registrable
domain. Both **requests** and **works** are counted.

**Ledger algorithm** (`trait Pacer` in `scitadel-core::ports`, SQLite
implementation in `scitadel-db`):

1. `spawn_blocking` → `BEGIN IMMEDIATE`.
2. Prune grants older than 24 h. If the bucket is in backoff, or the
   rolling 24 h window is at its cap, return `Denied`.
3. Otherwise reserve a slot at `max(now, next_allowed_ms)`, insert the
   grant, advance `next_allowed_ms`, and `COMMIT`.
4. **Sleep outside the transaction** until the slot. Permits count when
   they are granted, not when the fetch succeeds.

Further rules:

- `SQLITE_BUSY` means **no permit** (fail closed).
- An in-process `Semaphore(1)` per bucket keeps concurrency at 1.
- The batch scheduler round-robins across buckets, so a work waiting on
  Elsevier never holds up arXiv.
- Waits up to a threshold block; longer waits return `rate_limited`
  with `next_attempt_at` set.

**Defaults.** These can be tightened in config. There is no flag to go
looser.

| bucket class | min interval | cap (rolling 24 h) |
|---|---|---|
| OA metadata APIs (OpenAlex, Crossref, DataCite, Europe PMC, Unpaywall) | their documented limits (≥ 1 s default) | none beyond the API's |
| Springer platform (TDM) | 1 req/s | per `tdm_authorisation` |
| other tier-2 APIs | per the publisher's API terms | per the publisher's API terms |
| tier 4 (browser session) | ≥ 30 s + jitter per work | ≤ 50 works / publisher, ≤ 150 works total |

Caps apply per user. Institutional suspension is per publisher and hits
the whole institution, and scitadel cannot see other users at the same
institution. This is documented, not hidden.

## 5 · Credentials and the browser session

**TDM credentials** (tier 2) live in the existing credential store
(#213). They are wrapped in a `Secret` newtype with a redacting
`Debug`/`Display`, zeroize-on-drop and no `Serialize`.

- Each key is attached **only** to requests for its own publisher's
  bucket, and those requests don't follow redirects to other buckets.
- Every surface shows fingerprints only. A canary test plants a secret in
  each backend and asserts it never appears in MCP responses, CLI output,
  the TUI render buffer, TRACE logs, the manifest, the SQLite file or any
  error path.
- The macOS store must stop passing the value on argv first (#248).

**Tier 4, the browser session.** It sits behind the off-by-default
`browser-session` cargo feature and is implemented in Rust with
`chromiumoxide`.

- **Dedicated profile.** scitadel launches its own Chrome with
  `--user-data-dir=$XDG_DATA_HOME/scitadel/browser-profile`, and the user
  completes SSO there once. There is no option to attach to the user's
  everyday profile.
- **Endpoint.** `--remote-debugging-pipe`. If a port is unavoidable, it
  binds to `127.0.0.1` only, and non-loopback CDP URLs are rejected when
  the config loads. The endpoint comes from human-edited config only:
  never an environment variable or MCP parameter.
- **Origin allowlist per publisher.** It is enforced in Rust via
  `Fetch.enable` interception on every navigation, redirect and
  subresource-initiated navigation. DOIs are pre-resolved through the
  doi.org Handle API, and fetching is refused unless the target is
  allowlisted. Loopback, RFC 1918, link-local and `.local` are denied.
  URLs scraped from the page (`citation_pdf_url`, SI links) pass the same
  check.
- **Cookies stay in the browser.** Bodies are fetched in-page (`fetch`
  with `credentials:'include'` via `Runtime.evaluate`) or captured with
  `Fetch.getResponseBody`. `Network.getCookies` / `getAllCookies` are
  linted out.
- **Modes:**
  - **One-click (ships first).** The human opens a queued work in the
    scitadel browser. scitadel saves full text, SI, tables (JSON) and
    figure references from *that* page, runs the post-fetch identity
    check, then closes its own tab. One human action per work, and the
    pacer still applies.
  - **Bulk drain (gated).** Only for publishers that have a
    `tdm_authorisation` with `allows_bulk_session = 1`. It is started
    from the TUI or CLI by a human, **never through MCP**. Per-publisher
    auth probes detect login, mid-drain expiry pauses only that
    publisher, and the tier-4 pacing defaults apply.
- **Login detection** uses a cheap authenticated probe URL per
  publisher, never cookie-name heuristics.

**The agent surface (MCP).**

- `acquire_queue_add(paper_ids)` accepts only works whose DOI passed the
  pre-fetch identity check. There are no URLs, paths, output directories,
  publisher overrides or browser options. #249 removes the existing
  `download_paper.output_dir` the same way.
- Read-only verbs: `acquire_status`, `coverage`, `action_list`,
  `auth_status` (fingerprints only).
- `read_paper` returns full text wrapped in an explicit
  **untrusted-content envelope**, with scripts and styles stripped.
  Fetched content may contain prompt injection.

## 6 · Slice plan

Prerequisites (independent bugs): **#250** (transactional migrations),
**#248** (keychain argv), **#249** (MCP `output_dir`).

| # | slice | depends on | ships |
|---|---|---|---|
| S1 | **Model + pacer + importers** (step 2) | #250 | migration 013; `scitadel-acquire` crate with `PacedClient` + ledger + policy table; legacy migration; `import-flat`; `download_paper` rewired onto `Route` with no behaviour change; redirects paced |
| S2 | **acquire / coverage / action_list** (step 4) | S1 | CLI + MCP, idempotent and `--resume`/`--dry-run`; pre-fetch identity check; OSTI route; `scan`/`attach` drop-ins; manifest mirror; `drop_path` in the action list |
| S3 | **OA routes** (step 3) | S1 | resolve-then-rank; Europe PMC JATS + SI; PMC OA dataset; all OpenAlex locations; Crossref licence parsing; JATS tables → JSON |
| S4 | **Tier 4, one-click** (step 7a) | S1, #248, #249, browser spike | dedicated profile, pipe, allowlist, in-page fetch; save full text / SI / tables / figure refs from the opened page; post-fetch identity check |
| S5 | **SI + HTML tables** (step 6) | S3 | DataCite `IsSupplementTo`; publisher-hosted SI through publisher buckets; HTML tables → JSON; figure references |
| S6 | **Tier 2 TDM** (step 5) | S1, #248 | Elsevier / Wiley / Springer platform per §3; `tdm_authorisations`; TDM-permitted IP access only |
| S7 | **Tier 4 bulk drain + TUI queue** (step 7b) | S4, S6, library agreements | gated drain, per-publisher probes, TUI queue view with OSC 8 login links |

**Order for raid's first priority, an acquisition campaign:**
S1 → S2 → (S3 ∥ S4) → S5 → S6 → S7. S2 alone gives raid an inventory,
coverage and a grouped action list. S3 and S4 can run in parallel after
S1. raid retires `fulltext.py` after S1 + S2 + S3 + S6, and
`browser_cdp.py` after S4 + S5 (bulk mode after S7).

Out of scope: raid's `citation_lib.py` / `lint_dois.py`. That is a
repo-wide claim linter, not acquisition.

**Needs an empirical spike before S4:**
- whether `chromiumoxide` `Fetch`-domain body capture works with
  Chrome's PDF viewer and service workers;
- whether SWITCH edu-ID / Shibboleth SSO works in a fresh profile, and
  which IdP hosts the allowlist then needs.

## Alternatives considered

- **Manifest JSON as the source of truth.** Rejected: two writable
  stores drift, and cross-process updates need the DB's locking anyway.
- **A per-kind `acquisition_state` with a stored `have`.** Rejected: a
  second copy of the artefacts that drifts, and it can't represent
  several SI files or VoR and AM side by side.
- **Pacing per registrable host / UTC-day counters / `governor`.**
  Rejected:
  - publisher platforms span several hosts;
  - day counters allow twice the cap around midnight;
  - an in-memory limiter can't enforce caps across processes and
    restarts.
- **First-win-per-kind ladder with arXiv first.** Rejected: it stores
  preprints when an OA version of record exists.
- **A generic institutional-IP tier.** Rejected: scripted downloads from
  an institution's range are the "systematic downloading" licences
  forbid, and they are traced to the institution.
- **Attaching to the user's everyday browser (raid's approach).**
  Rejected: a confused deputy holding bank and mail sessions, and
  page-derived URLs can drive logged-in requests anywhere.
- **Keeping raid's Playwright fetcher as a sidecar.** Rejected: a second
  policy engine, a second place to configure the CDP endpoint, and a
  Python/Node supply chain with full browser control. Playwright's
  request context also most likely copies cookies into the driver
  (not verified; see #247).
- **Unrestricted bulk tier-4 draining.** Rejected: Swiss URG Art. 24d
  needs lawful access, and the licence terms decide. Bulk draining
  therefore needs a recorded per-publisher authorisation.
- **Tables as CSV.** Rejected: loses merged cells. CSV is an export view.

## Consequences

- One migration (013) plus a Rust backfill. The legacy columns are
  frozen, then dropped a release later.
- A new crate, `scitadel-acquire`, and new dependencies:
  `tokio-util` (cancellation) and `chromiumoxide` (feature-gated).
- Every outbound HTTP call in scitadel becomes paced, including metadata
  lookups. Search and `resolve_doi` may wait on their bucket.
- `read_paper` output changes shape: it adds an untrusted-content
  envelope.
- The library must be consulted before S7. S1–S6 don't depend on it.
