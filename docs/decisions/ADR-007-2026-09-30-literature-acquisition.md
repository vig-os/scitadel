# ADR-007 — Literature acquisition (works, artefacts, route ladder, pacer, access boundary)

**Status**: Accepted. S4 and S7 are conditional on the browser spike (§6) and, for S7, on a library agreement.
**Date**: 2026-09-30
**Supersedes**: —
**Context**: #247 (spike), #230 (paywall access ladder), #234 (corpus acquisition)

## Context

scitadel downloads one file per paper today. `download_paper`
(`crates/scitadel-adapters/src/download.rs`) walks arXiv → OpenAlex →
Unpaywall → publisher HTML → URL, and records the outcome in three
columns on `papers` (`local_path`, `download_status`, `last_attempt_at`;
migration 007). Its first consumer, raid (a radiopharmaceutical dataset
project), needs more than that:

- full text, supplementary information (SI), tables and figures for each
  work in a curated corpus;
- provenance and licence for each file;
- a coverage report, and one grouped list of what still needs a human;
- a way past paywalls that doesn't get the institution's access
  suspended.

As of 2026-09-29, 212 of raid's 587 DOIs are paywalled and no SI is on
disk.

#230 and #234 are two halves of one system. This ADR fixes the parts
every later slice depends on, so each slice can ship independently
against a stable contract. It reflects:

- round 1: five independent reviews (data architecture, research-library
  licensing, security, Rust async, and the raid consumer);
- the maintainer's decisions where those reviews disagreed;
- round 2: a traceability audit and an S1-implementer review.

The reviews and decisions are recorded on #247.

## Decision

1. **Data model.** A work is the existing `papers` row. It owns many
   **artefacts**: one row per file, pointing at a content-addressed
   **blob**. The database is the source of truth, and
   `papers/<stem>/manifest.json` is a generated, read-only mirror.
2. **One status vocabulary.** `acquisition_state` (per work, wanted kind
   and locator) is the only store behind coverage, `action_list` and
   `auth_pending`.
3. **Resolve, then rank.** For each work, a metadata pass (which touches
   no publisher hosts) finds every candidate location. The ladder then
   chooses by **version and licence**, not by whichever route answered
   first.
4. **One paced client.** Every outbound request, redirect hops included,
   goes through `PacedClient` (crate `scitadel-http`). Each request takes
   a permit from a **persistent, per-publisher-platform** ledger in
   SQLite. Routes cannot get at a raw HTTP client.
5. **Access tiers.**
   - Open access first.
   - Tier 2 is **publisher-sanctioned TDM only** (API or platform, on the
     publisher's own terms).
   - There is **no generic institutional-IP tier**.
   - Tier 4 runs in a **dedicated browser profile launched by scitadel**.
     It ships as **one-click "save what I opened"**. Bulk draining is
     gated on a per-publisher `tdm_authorisation`.
6. **Agent boundary.** Agents may queue works and read status and
   artefacts. They never supply URLs, paths, browser options or pacing,
   and never trigger browser fetching.

## 1 · Data model (migration 013)

### Storage

The **library root** is the absolute parent directory of the DB file.
An in-memory DB has no root; it gets no blobs and skips file backfills.

- Blobs live at `<root>/blobs/<first 2 hex of sha256>/<sha256>.<ext>`.
- Downloads are staged in `<root>/blobs/.tmp/`, on the same filesystem so
  they can be renamed atomically into place.
- Every stored path is relative to the root. Today's absolute
  `local_path` values break as soon as a library moves.

Size caps apply before a file is persisted and can be tightened in
config:

| artefact | cap |
|---|---|
| full text | 100 MB |
| a single SI file | 250 MB |
| figure | 20 MB |

A file over its cap is recorded as an attempt with outcome
`too_large`, and no artefact is created.

**Time.** Pacer fields are Unix epoch milliseconds (`*_ms`, INTEGER).
Every other timestamp is RFC 3339 UTC TEXT. Waits are computed as
durations and slept with `tokio::time::Instant`, never by subtracting
wall-clock values in a loop.

### Schema

The SQL runs inside the transactional migration runner (#250). The file
backfill (§1 "Legacy data") runs **after** the commit, as an idempotent
step of `Database::migrate`, next to `backfill_bibtex_keys`. That keeps
the write lock short, well under other processes' `busy_timeout`.

```sql
CREATE TABLE blobs (
    sha256     TEXT PRIMARY KEY,
    bytes      INTEGER NOT NULL,
    mime       TEXT,
    rel_path   TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE artefacts (
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
CREATE INDEX idx_artefacts_paper   ON artefacts(paper_id);
CREATE INDEX idx_artefacts_derived ON artefacts(derived_from);
CREATE INDEX idx_artefacts_sha     ON artefacts(sha256);

-- What we want and haven't got. There is no 'have' status: "have" is derived (see below).
CREATE TABLE acquisition_state (
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
CREATE TABLE acquisition_attempts (
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
CREATE INDEX idx_attempts_paper ON acquisition_attempts(paper_id);

CREATE TABLE acquisition_leases (
    paper_id       TEXT PRIMARY KEY REFERENCES papers(id) ON DELETE CASCADE,
    owner          TEXT NOT NULL,               -- pid + random nonce
    lease_until_ms INTEGER NOT NULL
);

CREATE TABLE paper_identity_checks (
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
CREATE INDEX idx_identity_paper ON paper_identity_checks(paper_id);

CREATE TABLE pacer_buckets (
    bucket           TEXT PRIMARY KEY,
    next_allowed_ms  INTEGER NOT NULL DEFAULT 0,
    backoff_until_ms INTEGER NOT NULL DEFAULT 0,
    backoff_reason   TEXT
);
CREATE TABLE pacer_grants (
    bucket         TEXT NOT NULL,
    tier           TEXT NOT NULL CHECK (tier IN ('meta','oa','tdm','session')),
    unit           TEXT NOT NULL CHECK (unit IN ('request','work')),
    granted_at_ms  INTEGER NOT NULL
);
CREATE INDEX idx_pacer_grants_bucket ON pacer_grants(bucket, granted_at_ms);
CREATE INDEX idx_pacer_grants_tier   ON pacer_grants(tier, unit, granted_at_ms);

CREATE TABLE tdm_authorisations (           -- no secrets here
    publisher           TEXT PRIMARY KEY,
    basis               TEXT NOT NULL CHECK (basis IN ('api_terms','library_agreement','platform_policy')),
    policy_ref          TEXT NOT NULL,
    allows_bulk_session INTEGER NOT NULL DEFAULT 0,
    max_works_per_day   INTEGER,              -- never raises a cap above the §4 defaults; can only tighten
    granted_by          TEXT NOT NULL,
    recorded_at         TEXT NOT NULL,
    expires_at          TEXT
);

ALTER TABLE papers ADD COLUMN pmcid TEXT;
ALTER TABLE papers ADD COLUMN osti_id TEXT;
CREATE UNIQUE INDEX idx_papers_pmcid ON papers(pmcid)   WHERE pmcid   IS NOT NULL;
CREATE UNIQUE INDEX idx_papers_osti  ON papers(osti_id) WHERE osti_id IS NOT NULL;
```

`pmcid` and `osti_id` join the `Paper` model and row mapping in S1.
Deduplicating on them lands with the routes that populate them (S3).

### "Have" (derived)

For a wanted kind of `fulltext`, the work *has* it when any
`fulltext_*` artefact exists with `access_status = 'full_text'`, and its
`version` satisfies `wanted_version`.

- `'any'` accepts every version.
- `'vor'` accepts `vor`. It also accepts `unknown`, but **only** on rows
  from `route IN ('legacy','import_flat')`, since those predate version
  tracking.
- A held version that doesn't satisfy the wanted one yields
  `wrong_version`.

For specific kinds (`si`, `table`, `figure`), the rule is the same with
the exact kind and locator.

### Artefact rules

- **Tables** are structured JSON (`format = 'json'`): caption, label,
  anchor, footnotes, and cells with `rowspan` / `colspan` / `th`. CSV is
  an export view, because CSV can't represent merged cells.
- **Figures** can be *references* (`sha256 IS NULL`, with `source_url`
  and `caption`). raid's digitisation queue needs the URL and caption
  without the image.
- **SI** only counts when its magic bytes match its declared format. The
  raid review found 7 of 27 "SI" files on disk were HTML landing pages.
- **Browser-tier HTML.** Rendered page HTML from the browser tier is
  **never stored** unless it *is* the full text (`kind = 'fulltext_html'`,
  with `access_status = 'full_text'` confirmed). Tables and figures
  extracted from it are stored as JSON and references.
- **Unreferenced blobs** are collected by `scitadel gc` (S2).

**Leases.** A work is claimed by the upsert below. No row returned means
another live process holds the lease. The holder renews while fetching.
Expired leases can be taken over, so a crashed process never blocks a
work for good.

```sql
INSERT INTO acquisition_leases (paper_id, owner, lease_until_ms) VALUES (?1, ?2, ?3)
ON CONFLICT(paper_id) DO UPDATE SET owner = excluded.owner, lease_until_ms = excluded.lease_until_ms
  WHERE acquisition_leases.lease_until_ms < ?4   -- ?4 = now_ms
RETURNING owner;
```

**The manifest mirror** is written only by the lease holder, after the
DB commit, via a temp file and a rename. It is never read as input.

**Manual drop-ins** are reconciled only by an explicit `scitadel scan`
or `scitadel attach <paper> <file> --kind …`, never as a side effect of
reading.

- Unknown files get `route = 'manual'`, `access_status = 'unknown'` and
  a post-fetch identity check.
- A file that has disappeared is flagged `missing_on_disk = 1`; the row
  stays.

### Legacy data

The backfill runs after the SQL migration, is idempotent (`INSERT OR
IGNORE` on the UNIQUE key), and **copies** files into `blobs/`. It never
moves them.

- `kind` is inferred from the file extension (`.pdf` →
  `fulltext_pdf`, `.html` → `fulltext_html`).
- Every legacy artefact gets `route = 'legacy'` and
  `access_basis = 'manual'`.
- The TUI has always collapsed Abstract / Paywall / Unknown into
  `paywall` (`app.rs` `persist_download_outcome`), so there is no real
  access status to preserve. The mapping is conservative:

| legacy state | result |
|---|---|
| `downloaded`, file exists | artefact `access_status = 'full_text'`, `version = 'unknown'` |
| `paywall`, file exists | artefact `access_status = 'unknown'`; state `fulltext` → `pending` |
| `paywall`, no file | state `fulltext` → `pending` |
| `failed` | state `fulltext` → `error`, `next_attempt_at` from `last_attempt_at` |
| recorded path no longer exists | no artefact; state `fulltext` → `pending` |

A **flat-layout importer**, `scitadel import-flat <dir>`, ingests files
written by consumers:

- raid's `<slug>.{pdf,html,tables.json,meta.json}` and `<slug>_SI_<name>`;
- `route = 'import_flat'`, with `imported_from` recorded, because a
  published datasheet cites those paths as provenance;
- `meta.json` figure URLs and captions become figure references;
- `tables.json` entries become table artefacts.

**Compatibility until S2.** S1 promises no behaviour change, so it keeps
**dual-writing**:

- the legacy columns `local_path`, `download_status` and
  `last_attempt_at`;
- the `papers/<stem>.<ext>` copy, which the TUI state column,
  `find_cached_file` and `read_paper` still read.

S2 moves the TUI and `read_paper` onto artefacts and stops the legacy
writes. The columns are dropped in the release after S2.

## 2 · Status vocabulary

These are the values of `acquisition_state.status`, enforced by the
schema's CHECK constraint.

| status | meaning | action_list group |
|---|---|---|
| `pending` | never tried | — (picked up by `acquire`) |
| `oa_fetchable` | an OA location is known, not yet fetched | — |
| `tdm_available` | a sanctioned TDM route exists and credentials are present | — |
| `tdm_key_missing` | a sanctioned TDM route exists, no credential | register a key |
| `needs_login` | tier 4 is possible, no live session | log in (scitadel browser) |
| `not_entitled` | authenticated, still 403 | ILL / library request |
| `needs_authorisation` | only bulk tier 4 could fetch it and no `tdm_authorisation` exists | open & save one-click, or ask the library |
| `needs_ill` | no route (includes works without a DOI) | ILL request with an exported citation |
| `unavailable` | the work has no such artefact (e.g. no SI) | — |
| `identity_mismatch` | the resolved or served title doesn't match the expected one | check DOI (both titles shown) |
| `wrong_version` | only a non-wanted version is held | fetch the wanted version |
| `rate_limited` | the bucket is in backoff or at its cap | — (retry at `next_attempt_at`) |
| `error` | transient failure | — (retry with backoff) |

`action_list` groups rows by **(action group, publisher)** and carries
`drop_path` and `hint_url`. Its counts sum exactly to the missing totals
in `coverage`.

## 3 · Routes and the ladder

**Crates and dependency direction** (no cycles):

- `scitadel-core`: domain types, the `Pacer` port, statuses.
- `scitadel-http`: `PacedClient`, the **only** crate allowed to name
  `reqwest::Client`. Depends on core.
- `scitadel-db`: the SQLite `Pacer`, artefacts and the ledger. Depends
  on core.
- `scitadel-adapters`: metadata and search adapters. Depends on core and
  http.
- `scitadel-acquire`: the `Route`s, the ladder, the importers and the
  blob store. Depends on core, http and adapters, and does **not**
  depend on `reqwest`.

**Enforcement.** A workspace `clippy.toml` sets
`disallowed-types = ["reqwest::Client", "reqwest::ClientBuilder"]` and
`disallowed-methods = ["reqwest::get"]`.

- `scitadel-http` allows these in `paced.rs`.
- Existing adapters that still build their own client (PubMed, INSPIRE,
  EPO, Lens, etc.) get a per-file `#[allow(clippy::disallowed_types)]`
  carrying a `TODO(#247-S3)`.
- The download chain moves onto `PacedClient` in S1. The metadata
  adapters that acquisition calls (OpenAlex, Crossref, DataCite,
  Unpaywall, Europe PMC) move in S3. Search-only adapters move in a
  later slice, filed when S3 lands.

**Publishing.** `scitadel-http` and `scitadel-acquire` join the
workspace's shared version and the crates.io publish order
(`publish-crates.yml`):

1. core
2. **http**
3. db
4. adapters
5. **acquire**
6. scoring
7. export
8. mcp
9. tui
10. cli

```rust
pub struct Work {                     // read-only view over papers + identity
    pub paper_id: String,
    pub doi: Option<String>, pub pmcid: Option<String>, pub osti_id: Option<String>,
    pub arxiv_id: Option<String>, pub openalex_id: Option<String>,
    pub title: String, pub year: Option<i32>,
}
pub struct RouteCtx {
    pub client: PacedClient,          // the only way to issue requests
    pub blobs: BlobStore,             // temp → verify sha256 + magic + size → persist
    pub cancel: CancellationToken,
}
#[async_trait]
pub trait Route: Send + Sync {
    fn id(&self) -> RouteId;
    fn tier(&self) -> Tier;                            // Oa | Tdm | Session
    fn likely_buckets(&self, w: &Work) -> Vec<Bucket>; // --dry-run coverage only; never a gate
    fn applies(&self, w: &Work, want: &KindSet) -> bool;
    async fn fetch(&self, cx: &RouteCtx, w: &Work, want: &KindSet) -> RouteOutcome;
}
pub struct RouteOutcome {
    pub got: Vec<NewArtefact>,
    pub status: BTreeMap<Kind, AcqStatus>,   // the ladder asserts got ∪ status covers `want`
}
pub type Ladder = Vec<Arc<dyn Route>>;
```

`#[async_trait]` is used because the ladder is a trait-object `Vec`, and
`SourceAdapter` already works this way. Routes must be cancel-safe:
write to the temp file and persist only after verification.

**Resolve, then rank.** One metadata pass per work, touching no
publisher hosts, gathers:

- OpenAlex (all `locations[]`);
- Unpaywall;
- Crossref (`license[]`, `link[intended-application=text-mining]`,
  relations);
- DataCite;
- Europe PMC (`isOpenAccess`, `inEPMC`).

Candidates are ranked **OA VoR > AM > preprint**, then by licence
strength. Crossref `license[]` counts as OA only when it is an
allow-listed Creative Commons URL that is in force today (considering
`content-version`, `start` and `delay-in-days`). ACS's own policy URLs
(`10.15223/policy-*`) don't count.

**Fetch order** within the ranked candidates:

1. **Europe PMC:** `fullTextXML` (JATS) and `supplementaryFiles`, only
   for OA records or author manuscripts. Author manuscripts get
   `version = am` and `license_url = NULL`. `inEPMC = Y` alone means free
   to read, not a reuse licence.
2. **The PMC OA dataset** (`pmc-oa-opendata` S3, anonymous HTTPS). The
   `oa.fcgi` service was retired on 2026-08-25. Take the licence and
   `is_manuscript` from the per-version JSON. Don't assume the package
   contains SI until its README confirms it.
3. **OA repository locations** from OpenAlex and Unpaywall, arXiv for
   preprints, and **DOE OSTI** for national-lab reports.
4. **SI discovery:**
   - DataCite `IsSupplementTo`, which covers Taylor & Francis supplements
     on Figshare, plus Zenodo and Dryad;
   - a cheap Crossref `is-supplemented-by` check, which is ≈0 for ACS
     and RSC;
   - SI hosted by the publisher (ACS, RSC) is fetched through the
     **publisher's** bucket.
5. **Tier 2 (sanctioned TDM):**
   - **Elsevier:** the Article Retrieval and Object APIs, for full text
     and `mmc` SI. API key, plus `insttoken` off campus. Non-commercial
     use.
   - **Wiley:** TDM token.
   - **Springer Nature:** platform download at ≤ 1 req/s. The Springer
     API key alone only unlocks the OA API.
   - **ACS, RSC, T&F, SNMMI:** no self-serve API; they need a
     library-negotiated `tdm_authorisation`.
   - **ScienceDirect is never scraped.**
6. **Landing-page HTML**, for OA works only, after TDM, and never for
   ScienceDirect.
7. **Tier 4** (§5).
8. Otherwise the work lands in `needs_ill`, `needs_authorisation` or
   `not_entitled`, with a `drop_path`.

**Identity** is checked twice, and a mismatch blocks filing anything
under that DOI.

- **Pre-fetch**, in S2 for every route: the expected title against the
  resolved one (OpenAlex → Crossref → DataCite), fuzzy-matched with
  year ±1 and the first author as tie-breakers.
- **Post-fetch**, in S2 for HTTP routes and S4 for the browser: the
  served page or PDF title against OpenAlex. This catches redirects to
  the wrong paper.

`doi_corrected_from` is recorded whenever a DOI is fixed.

## 4 · Pacer

```rust
// scitadel-core::ports
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)] pub struct Bucket(pub String);
pub enum PaceTier { Meta, Oa, Tdm, Session }
pub enum Cost { Request, Work }
pub struct Permit { pub bucket: Bucket, pub tier: PaceTier, pub not_before: std::time::Instant }
pub enum PaceDenied { DailyCap, Backoff { until_ms: i64 }, LedgerBusy, WaitTooLong { until_ms: i64 } }
#[async_trait]
pub trait Pacer: Send + Sync {
    async fn acquire(&self, b: &Bucket, tier: PaceTier, cost: Cost) -> Result<Permit, PaceDenied>;
}
// scitadel-http
pub struct PacedClient { /* private reqwest::Client (redirect = Policy::none()), Arc<dyn Pacer>, BucketPolicy */ }
impl PacedClient {
    pub async fn get(&self, url: Url, tier: PaceTier, headers: SafeHeaders) -> Result<PacedResponse, FetchError>;
}
```

**Redirects.** `PacedClient` follows redirects itself and takes **one
`Request` permit per hop**, keyed to that hop's bucket. A redirect to a
known login page becomes `FetchError::LoginRedirect`, which records
`needs_login` and puts the bucket into backoff.

**Buckets are publisher platforms, not hosts.** A policy table maps
hosts to buckets. For example, `sciencedirect.com`,
`pdf.sciencedirectassets.com`, `ars.els-cdn.com`,
`linkinghub.elsevier.com` and `api.elsevier.com` are all `elsevier`. A
host that isn't in the table gets a bucket named after its registrable
domain, with the "unknown host" defaults below.

**Work grants.** One `Work` grant is taken per work per bucket, on the
first hop into that bucket for that work. Every hop also takes a
`Request` grant.

**The ledger.** The SQLite implementation lives in `scitadel-db`.

1. `spawn_blocking`, then `BEGIN IMMEDIATE`.
2. Prune grants older than 24 h.
3. If the bucket is in backoff, or its rolling 24 h window (per tier and
   unit, plus the cross-publisher session total) is at the cap, return
   `Denied`.
4. Otherwise reserve a slot at `max(now, next_allowed_ms)`, insert the
   grant, advance `next_allowed_ms`, and `COMMIT`.
5. **Sleep outside the transaction** until `not_before`.

Rules around the ledger:

- A permit counts when it is **granted**, not when the fetch succeeds.
- `SQLITE_BUSY` means **no permit**: fail closed.
- An in-process `Semaphore(1)` per bucket keeps concurrency at 1.
- The batch scheduler round-robins across buckets, so a work waiting on
  Elsevier never holds up arXiv.
- Waits up to **10 s** block. Longer waits return `WaitTooLong`, which
  becomes `rate_limited` with `next_attempt_at`.

**Defaults.** These are policy choices, not publisher-published limits.
Config can make any of them stricter; there is no setting that loosens
them, and a `tdm_authorisation` can only tighten a cap, never raise it.

| bucket class | min interval | cap (rolling 24 h) |
|---|---|---|
| metadata APIs (OpenAlex, Crossref, DataCite, Europe PMC, Unpaywall) | the API's documented limit, entered in the policy table (no extra floor) | the API's documented limit |
| OA repositories and arXiv | 3 s per request | 1,000 requests |
| publisher hosts, non-TDM (OA landing HTML, publisher-hosted SI) | 10 s per request | 300 requests / 100 works |
| Springer platform (TDM) | 1 s per request | per `tdm_authorisation` |
| other tier-2 APIs | per the publisher's API terms | per the publisher's API terms |
| tier 4 (browser session) | ≥ 30 s + jitter per work | ≤ 50 works per publisher, ≤ 150 works total |
| unknown host | 5 s per request | 500 requests |

Caps are **per user**. Institutional suspension happens per publisher
and affects the whole institution, and scitadel cannot see other users.
This limitation is documented, not hidden.

## 5 · Credentials and the browser session

**TDM credentials** (tier 2) use the existing credential store (#213).
The #248 fix, which stops the macOS backend putting secrets on the
command line, lands first.

- **In memory:** values are wrapped in a `Secret` newtype, with a
  redacting `Debug`/`Display`, zeroize-on-drop and no `Serialize`.
- **Scope:** a key is attached **only** to requests in its own
  publisher's bucket, and those requests don't follow redirects to other
  buckets.
- **Output:** every surface shows fingerprints only.
- **Canary test:** a secret planted in each backend must never appear
  in:
  - MCP responses, CLI output or the TUI render buffer;
  - TRACE-level logs, the manifest or the SQLite file;
  - any error path.

**Tier 4 (browser session).** It is off by default, behind the
`browser-session` cargo feature, and written in Rust with
`chromiumoxide`.

- **Dedicated profile.** scitadel launches its own Chrome with
  `--user-data-dir=$XDG_DATA_HOME/scitadel/browser-profile`, and the user
  does their SSO there once. There is no option to attach to an everyday
  profile.
- **Endpoint.** `--remote-debugging-pipe`. If a port is unavoidable, it
  binds `127.0.0.1` only, and config loading rejects any non-loopback CDP
  URL. The endpoint can only be set in human-edited config: never through
  an environment variable or an MCP parameter.
- **Origin allowlist per publisher**, enforced in Rust:
  - `Fetch.enable` interception covers every navigation, redirect and
    subresource-initiated navigation;
  - DOIs are pre-resolved through the doi.org Handle API, and fetching is
    refused unless the target is allowlisted;
  - loopback, RFC 1918, link-local and `.local` addresses are denied;
  - URLs scraped from the page (`citation_pdf_url`, SI links) pass the
    same check.
- **Cookies stay in the browser.** Bodies are fetched from inside the
  page (`fetch` with `credentials: 'include'` via `Runtime.evaluate`), or
  captured with `Fetch.getResponseBody`. `Network.getCookies` and
  `getAllCookies` are linted out.
- **One-click mode** (S4) ships first. The human opens a queued work
  (from the S2 queue) in the scitadel browser. scitadel then saves from
  *that* page:
  - full text;
  - SI;
  - tables as JSON, via the HTML table extractor, which S4 introduces and
    S5 reuses;
  - figure references.

  It then runs the post-fetch identity check and closes its own tab. One
  human action per work, and the pacer still applies.
- **Bulk drain** (S7) is gated:
  - only for publishers with a `tdm_authorisation` where
    `allows_bulk_session = 1`;
  - started by a human from the TUI or CLI, **never through MCP**;
  - a per-publisher authenticated probe detects login;
  - a session expiring mid-drain pauses only that publisher;
  - the tier-4 pacing defaults apply.

**Agent surface (MCP).**

- `acquire_queue_add(paper_ids)` accepts only works whose DOI passed the
  pre-fetch identity check. There are no URLs, paths, output directories,
  publisher overrides or browser options. #249 removes the existing
  `download_paper.output_dir` in the same spirit.
- Read-only verbs: `acquire_status`, `coverage`, `action_list` and
  `auth_status` (fingerprints only).
- `read_paper` returns full text wrapped in an explicit
  **untrusted-content envelope**, with scripts and styles stripped.
  Fetched content may carry prompt injection.

## 6 · Slices

Three independent bugs are prerequisites: **#250** (transactional
migrations), **#248** (keychain secret on the command line) and **#249**
(MCP `output_dir`).

| # | slice | depends on | acceptance |
|---|---|---|---|
| S1 | Model + pacer + importers | #250 | migration 013 plus the backfill (idempotent: a second run changes nothing); `scitadel-http` with `PacedClient` and the SQLite ledger; the clippy lint is on for the workspace; a two-process test shows the cap isn't exceeded; a wiremock redirect test (A→302→B) shows two permits; `download_paper` runs on `Route` + `PacedClient` with the same outputs as today (dual-write); `import-flat` imports a fixture of raid's layout losslessly (tables, figure refs, `imported_from`) |
| S2 | `acquire` / `coverage` / `action_list` / queue | S1 | CLI + MCP, idempotent, `--resume` and `--dry-run`; pre- and post-fetch identity checks for HTTP routes; OSTI route; `scan` / `attach`; manifest mirror; `gc`; TUI and `read_paper` move onto artefacts and legacy writes stop; `action_list` counts equal the `coverage` missing totals; raid's acquisition fields in its `p1`/`p3` NDJSON queues import via `acquire --from works.ndjson` (curation statuses stay raid's) |
| S3 | OA routes | S1 | resolve-then-rank; Europe PMC JATS + SI; PMC OA dataset; OpenAlex locations; Crossref licence parsing; JATS tables → JSON (`table-wrap` count equals table artefacts); acquisition metadata adapters on `PacedClient` |
| S4 | Tier 4, one-click | S2, #248, #249, browser spike | dedicated profile, pipe, allowlist, in-page fetch; save full text / SI / tables (JSON) / figure refs from the opened page; post-fetch identity check; the allowlist blocks a non-publisher redirect in a test |
| S5 | SI + HTML tables at scale | S3, S4 | DataCite `IsSupplementTo`; publisher-hosted SI through publisher buckets with magic-byte checks; HTML tables → JSON for OA landing pages (reusing S4's extractor); pacing covers SI (tested) |
| S6 | Tier 2 TDM | S1, #248 | Elsevier / Wiley / Springer platform per §3; `tdm_authorisations`; TDM-permitted IP access only; canary non-leakage test passes |
| S7 | Tier 4 bulk drain + TUI queue | S4, S6, a library agreement | gated drain; per-publisher probes; expiry pauses one publisher only; TUI queue view with OSC 8 login links; caps enforced and visible |

**Order for raid's first priority**, an acquisition campaign:
S1 → S2 → (S3 ∥ S4) → S5 → S6 → S7.

- S2 alone gives raid an inventory, coverage and a grouped action list.
- raid retires `fulltext.py` after S1 + S2 + S3 + S6.
- raid retires `browser_cdp.py` after S4 + S5, and its bulk mode after
  S7.
- raid's `citation_lib.py` / `lint_dois.py` are out of scope: they are a
  repo-wide claim linter, not acquisition.

**Empirical spike before S4:**

- Does `chromiumoxide`'s `Fetch`-domain body capture work with Chrome's
  PDF viewer and with service workers?
- Does SWITCH edu-ID / Shibboleth SSO work in a fresh profile, and which
  IdP hosts does the allowlist then need?

## Alternatives considered

- **Manifest JSON as the source of truth.** Rejected: two writable
  stores drift, and updates across processes need the DB's locking
  anyway.
- **Per-kind state with a stored `have`.** Rejected: it duplicates the
  artefacts and drifts, and it can't represent several SI files or VoR
  and AM side by side.
- **Pacing per registrable host / UTC-day counters / `governor`.**
  Rejected:
  - a publisher platform spans several hosts;
  - a day counter allows twice the cap around midnight;
  - an in-memory limiter can't enforce caps across processes and
    restarts.
- **A paced client inside the routes crate.** Rejected: the metadata
  adapters need it too, which would create a dependency cycle.
  `scitadel-http` sits below both.
- **First-win-per-kind with arXiv first.** Rejected: it stores preprints
  when an OA version of record exists.
- **A generic institutional-IP tier.** Rejected: scripted downloads from
  an institution's IP range are exactly the "systematic downloading"
  licences forbid, and they are traced back to the institution.
- **Attaching to the user's everyday browser (raid's current approach).**
  Rejected: a confused deputy that holds bank and mail sessions, where
  page-derived URLs can drive logged-in requests anywhere.
- **Keeping raid's Playwright fetcher as a sidecar.** Rejected:
  - a second policy engine and a second place to configure the CDP
    endpoint;
  - a Python/Node supply chain with full control of the browser;
  - Playwright's request context most likely copies cookies into the
    driver (not verified; #247).
- **Unrestricted bulk tier-4 draining.** Rejected on the licensing
  review's reading:
  - Swiss URG Art. 24d requires lawful access, and appears to have no
    clause that overrides contracts. This was from memory, not verified
    against Fedlex.
  - EU DSM Art. 3 doesn't cover a Swiss institution.

  Licence terms therefore decide, and bulk draining needs a recorded
  per-publisher authorisation.
- **Tables as CSV.** Rejected: loses merged cells. CSV is an export view.

## Consequences

- Migration 013 plus a post-commit backfill. Legacy columns are
  dual-written until S2 and dropped a release later.
- **Two new published crates**, `scitadel-http` and `scitadel-acquire`,
  added to the publish order. New dependencies: `tokio-util`
  (cancellation) and `chromiumoxide` (feature-gated).
- A workspace clippy lint on raw `reqwest` usage. Existing adapters are
  exempted per file until S3 or later.
- Every outbound HTTP call in acquisition is paced, including metadata
  lookups. Search and `resolve_doi` wait on their bucket once they move
  onto `PacedClient`.
- `read_paper` output changes shape: it gains an untrusted-content
  envelope (S2).
- The library is consulted before S7. S1–S6 don't depend on it.
