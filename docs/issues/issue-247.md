---
type: issue
state: closed
created: 2026-09-30T00:07:17Z
updated: 2026-09-30T00:39:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/247
comments: 4
labels: feature, effort:medium, phase:design
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:23.687Z
---

# [Issue 247]: [spike(acquisition): joint design — work/artefact model, route ladder, persistent pacer, credential + CDP isolation boundary (#230 + #234 step 1)](https://github.com/vig-os/scitadel/issues/247)

## Motivation

#230 (paywall access ladder) and #234 (corpus acquisition) are one system. They need **one** data model, **one** route ladder, **one** pacer and **one** credential/isolation boundary. Build those piecemeal across seven slices and we'll migrate the data model twice and retrofit pacing onto routes that already hit publisher hosts.

This spike fixes those foundations as an ADR before any slice lands.

**What changes if this lands:** steps 2–7 of the plan, below, become independent, reviewable PRs against a fixed contract, and several can run in parallel. The two things that are expensive to get wrong are settled first:
- the persistent data model, a migration every user carries;
- the safety envelope. A pacing mistake can get an **institution's** access suspended, for everyone at that institution, not just this user.

**Measured need (raid, 2026-09-29):** 212 of 587 DOIs are paywalled. Supplementary information on disk is effectively zero. 8,851 paper rows exist, almost all metadata-only. One cited set is 1,038 DOIs with 20 full texts. raid holds ~3k lines of working Python that should move upstream.

## Decisions to make (proposed answers — reviewers, please refute)

### D1 — Data model: work → artefacts
- A new `artefacts` table (migration 013), one row per file: `id, paper_id → papers.id, kind, format, path, route, version (vor|am|preprint|unknown), license, license_source, sha256, bytes, source_url, retrieved_at, access_status, derived_from → artefacts.id, locator, label`.
- `kind ∈ {fulltext_pdf, fulltext_html, fulltext_xml, si, table, figure}`.
- The work stays the existing `papers` row. It gains identity columns as needed (`pmcid`, `osti_id`, a `report_number` / `isbn` / `url` fallback) and an `identity_check` record (`expected_title`, `resolved_title`, `score`, `status`).
- **Missing artefacts are rows too**: an `acquisition_state` table keyed by `(paper_id, kind)`, holding a status from the shared vocabulary (D2), `reason`, `publisher`, `hint_url`, `last_attempt_at` and `attempts`. "What's missing" is a query, not a scan.
- **The DB is the source of truth.** `papers/<stem>/manifest.json` is a generated, read-only **mirror**, rewritten after every change for consumers without DB access. It is never read back as input, except for one case: a manual drop-in is detected as a *file* in the work directory, not as manifest content.
- Migration: existing `papers.local_path` / `download_status` rows become artefacts with `route = legacy` and `kind` inferred from the extension. The old columns stay readable for one release, then are dropped.

### D2 — One status vocabulary (per `(work, kind)`)
`have · oa_fetchable · tdm_available · tdm_key_missing · needs_login · needs_ill · unavailable · identity_mismatch · rate_limited · error`

This is the only vocabulary. #230's `auth_pending()` and #234's `action_list()` are **views** over `acquisition_state`, not separate stores. `action_list` groups the rows by `(reason, publisher)`, and its counts must sum to the `missing` totals in `coverage`.

### D3 — Route trait and ladder
```rust
#[async_trait]
trait Route {
    fn id(&self) -> RouteId;                 // europepmc, pmc_oa, osti, openalex_locations, unpaywall, arxiv, landing_html, crossref_relation, datacite, tdm_elsevier, …, browser_session
    fn tier(&self) -> Tier;                  // Oa | Tdm | InstitutionalIp | HumanSession
    fn hosts(&self, work: &Work) -> Vec<Host>;   // declared up front, so the pacer can gate every request
    fn applies(&self, work: &Work, want: &[Kind]) -> bool;
    async fn fetch(&self, ctx: &RouteCtx, work: &Work, want: &[Kind]) -> RouteOutcome; // artefacts found + per-kind status for what it couldn't get
}
```
- Every HTTP request goes through `RouteCtx`'s client, which **requires a pacer permit** for the target host. Bypassing the pacer shouldn't be expressible; a test asserts no route owns a raw `reqwest::Client`.
- The ladder is ordered, first win per kind. A route can yield several kinds: a PMC OA package gives full text, SI and figures at once. Ladder order: arXiv → Europe PMC (JATS) → PMC OA package → OpenAlex (all `locations[]`) → Unpaywall → OSTI → Crossref relation / DataCite (SI) → landing HTML (+ SI links) → TDM → institutional IP → human session → `needs_manual` / `needs_ill`.
- `download_paper` becomes `acquire(work, [fulltext_*])` over the same ladder; the CLI and MCP keep their behaviour.

### D4 — Pacer: persistent, per host, on by default, hard to disable
- A token bucket per **registrable host**, with a per-publisher policy table: min interval, burst and **daily cap**. Publisher hosts default to raid's baseline, ≥20 s between works on a publisher host and a daily cap. API hosts (Europe PMC, OpenAlex, Crossref) get their documented limits.
- **State lives in SQLite** (`pacer_ledger(host, day, count, last_at)`). The cap survives restarts and is shared by CLI, MCP and TUI processes, so a restart doesn't reset the cap. Implemented as a check-and-increment inside a transaction.
- Policies can be *tightened* in config. *Loosening* a publisher host below the floor requires an explicit, logged override flag. Nobody lowers the floor by accident.
- A 429/403 or a redirect-to-login trips a per-host backoff and records `rate_limited` / `needs_login`, never retrying hot.

### D5 — Credentials (tier 2)
- Reuse the #213 store (macOS Keychain / Secret Service / 0600 file; `auth status` shows fingerprints only). Add per-publisher TDM keys as `SourceCredentials` entries (`elsevier.api_key` + `elsevier.insttoken`, `springer.api_key`, `wiley.tdm_token`).
- Values are only ever read inside the TDM route's request builder. Every surface (MCP, CLI, logs, errors) shows fingerprints, and a test greps all of them for a planted secret.

### D6 — Tier 4 isolation boundary: cookies never leave the browser
- scitadel **never holds session cookies**. It attaches over CDP to the **user's own running browser**, opens **one tab**, navigates `doi.org/<doi>`, and fetches the PDF and SI through the page's own context (`context.request`), the way raid's `browser_cdp.py` does. The agent-facing surface returns statuses and artefacts only.
- **Fork: Rust port vs sidecar.** Port to Rust (`chromiumoxide` 0.9 or `headless_chrome`, both CDP-attach capable), or keep raid's Playwright-Python fetcher as a sidecar that scitadel drives through a narrow interface ("fetch DOI X, write to dir Y, report"). *Proposal: Rust port behind a `browser-session` cargo feature.* One binary, one pacer and one ledger; the sidecar would need its own pacing, or a second path to the ledger.
- Honest boundary statement for the docs: CDP access **is** full control of that browser profile. Anything with the CDP endpoint can read the cookies. So the CDP endpoint is configured by the human only, never through an MCP verb. The boundary is *enforced for agents* (no verb exposes cookies or the endpoint) and *disciplined for the local user*.
- Login completion is detected by a **per-publisher authenticated probe** (a cheap known URL whose success means "in"), never cookie-name heuristics.

### D7 — Identity check
- Resolve the title through OpenAlex (existing `resolve_doi`, #238), then Crossref, then DataCite. Fuzzy-match against `expected_title`, with year ±1 and first author as tiebreaks. Normalisation reuses the `bib_verify` path.
- `identity_mismatch` blocks filing *any* artefact under that DOI. Both titles are reported.

## What already exists
- `crates/scitadel-adapters/src/download.rs`: `AccessStatus` (L26), `DownloadResult` (L50), `detect_access_status()` (L69), the `download_paper()` ladder (L183: arXiv → OpenAlex → Unpaywall → publisher HTML → URL), `file_stem_for()` (L582).
- Migrations `003_full_text.sql` (`papers.full_text`) and `007_paper_download_state.sql` (`local_path`, `download_status`, `last_attempt_at`); the latest is 012.
- `crates/scitadel-core/src/credentials.rs`: backend detection, `SourceCredentials`, fingerprint-only `auth status` (#213).
- `resolve_doi` / title search (#238); ID prefix resolution (#237); `bib_verify` / `bib_snapshot`; `crates/scitadel-mcp/src/extract.rs` (`pdftotext -layout`).
- raid prior art to port:
  - `tools/fetch/fulltext.py` (936 lines): routes A–E, per-host rate limiter, `tdm_key_missing`;
  - `tools/fetch/browser_cdp.py` (778 lines): CDP attach, one tab, ≥20 s floor, table/SI/figure extraction, title check;
  - `scripts/citation_lib.py` + `lint_dois.py`: DOI↔title verification.

## Scope

**P0 — this spike**
- [ ] ADR `docs/decisions/ADR-007-…-literature-acquisition.md` fixing D1–D7
- [ ] Migration 013 schema written out in the ADR, not yet applied
- [ ] The `Route` trait, `RouteCtx` and the pacer API as a compiling Rust sketch, types only
- [ ] A slice plan for steps 2–7, each slice independently shippable with its own acceptance criteria

**Slice plan (to be refined by the ADR)**
1. Step 2: migration 013 + `artefacts` / `acquisition_state` + pacer ledger + legacy migration; `download_paper` rewired onto `Route` with no behaviour change
2. Step 3: Europe PMC JATS, PMC OA package, OSTI and OpenAlex `locations[]` routes
3. Step 4: `acquire` / `coverage` / `action_list` (CLI + MCP) + identity check + `--resume` / `--dry-run`
4. Step 5: tier-2 TDM (Elsevier / Springer / Wiley) + institutional-IP detection
5. Step 6: SI (Crossref relation, DataCite, JATS, landing-page selectors) + JATS/HTML tables to CSV
6. Step 7: tier-4 CDP session + `auth_pending` / `fetch_drain` + TUI queue view

## Pitfalls
1. **Institution-wide suspension.** Pacing must be a persistent default, not a flag, and must also cover SI harvesting and identity-check lookups that hit publisher hosts.
2. **Publisher ToS vs TDM rights.** Many licences forbid systematic downloading however you're authenticated. Tier 2 (TDM APIs) and statutory TDM exceptions (EU DSM Art. 3 for research organisations) are the defensible routes. Tier 4 is the *least* defensible and ships last, opt-in per publisher.
3. **CDP is total control of the user's profile:** banking and mail sessions included. The CDP endpoint must never be reachable or configurable by an agent.
4. **Manifest drift** if both the DB and JSON are writable. Hence D1: the JSON is a mirror only.
5. **Legacy migration ambiguity:** existing HTML rows can be full text, an abstract or a paywall page. Keep `access_status` and don't upgrade `Unknown` to `have`.
6. **Identity-check false positives** on short or generic titles ("Introduction", "Erratum"), transliterated names, or conference vs journal versions. They need a manual override that is recorded.
7. **Mid-drain session expiry is the normal case.** Pause that publisher's slice only.
8. **Multi-process races:** CLI, MCP and TUI running at once must not double-fetch or overrun a cap. The pacer and the acquisition queue need row-level claiming.
9. **Storage growth:** SI zips and figures. Content-address by sha256, dedupe, and record sizes.
10. **Assuming the big three publishers:** report route coverage per corpus *before* fetching.

## Acceptance criteria
- [ ] ADR merged with D1–D7 decided; each rejected alternative has a stated reason
- [ ] The migration 013 schema and `Route` / pacer type sketch compile (`cargo check`) behind a feature or in a doc-test
- [ ] Every slice has a concrete acceptance list, and the ordering says what's parallelisable
- [ ] The raid maintainer confirms the model covers raid's queue files (`p1` / `p3` NDJSON) without loss

## References
#230, #234, #246 (consumes JATS full text), #238 (`resolve_doi`), #237 (ID resolution), #213 (credential store), #112 (download state), #50 (institutional-access hint); raid `tools/fetch/fulltext.py`, `tools/fetch/browser_cdp.py`, `scripts/citation_lib.py`, `scripts/lint_dois.py`; crates `chromiumoxide`, `headless_chrome`, `governor`.

---

# [Comment #1]() by [gerchowl]()

_Posted on September 30, 2026 at 12:12 AM_

## Review round 1: consolidated (5 independent reviews)

Reviewers: data/persistence architect, research-library TDM and licensing specialist, application-security engineer, Rust async engineer, and raid's data curator (the first consumer). **Verdicts:** 3 × BLOCK as written (security on D6, licensing, raid), 2 × approve with changes (data, Rust). Nobody rejected the core shape: work → artefacts, the DB as the source of truth, one status vocabulary, one paced client, Rust over a sidecar. What's wrong is the details, and they're fixable.

### Where the reviewers agree (take as decided)
1. **Pace per publisher platform, not per registrable host** (data, licensing). `sciencedirect.com`, `pdf.sciencedirectassets.com`, `ars.els-cdn.com` and `api.elsevier.com` share one Elsevier bucket. Count *requests* as well as *works*, since SI multiplies requests per work.
2. **The pacer is unbypassable only if we turn off redirect following** (Rust). reqwest's default policy follows up to 10 hops to any host, which is also a hole in the existing downloader (`download.rs:138`). `PacedClient` uses `redirect(Policy::none())`, follows hops itself, and takes a permit per hop. Routes live in their own crate with no `reqwest` dependency, plus a clippy `disallowed-types` / `disallowed-methods` lint. The OpenAlex and Crossref adapters (identity and `resolve_doi` lookups) move onto `PacedClient` too.
3. **Ledger mechanics** (data, Rust):
   - `BEGIN IMMEDIATE` (the repo only uses deferred transactions today, which gives `SQLITE_BUSY_SNAPSHOT` under WAL);
   - reserve the slot, commit, then sleep *outside* the transaction;
   - count the permit when granted;
   - **fail closed** on `SQLITE_BUSY`;
   - a rolling 24 h window instead of a UTC-day counter, which would allow 2× the cap around midnight;
   - no `governor`: in-process `Semaphore(1)` per bucket plus the ledger;
   - round-robin fairness across buckets.
4. **Rust `chromiumoxide`, behind the `browser-session` feature, not a Playwright sidecar** (security, Rust): one process, one allowlist, one pacer, one audit log. Correction to D6: Playwright's `context.request` most likely copies cookies into the driver process (flagged as not verified). Fetch in the page itself (`fetch(…, {credentials:'include'})` via `Runtime.evaluate`) or capture responses (`Fetch.getResponseBody`); lint `get_cookies` out.
5. **Tier-4 queue draining is not an agent capability** (security, licensing). Agents may queue `paper_id`s and read status. Draining needs a human action in the TUI/CLI, off by default.

### Where they disagree or add constraints: the decisions for the human
- **D6 profile: the user's everyday browser vs a dedicated one.** The security reviewer blocks attaching to the everyday profile: it's a confused deputy with bank and mail sessions, and scraped `citation_pdf_url` or SI hrefs can drive logged-in GETs to arbitrary origins. It asks for a scitadel-launched Chrome with its own `--user-data-dir` (SSO done there once), `--remote-debugging-pipe` or a loopback-only endpoint (raid's default `100.94.46.106:9222` exposes the whole profile to the tailnet), and a per-publisher **origin allowlist** enforced with `Fetch.enable`, pre-resolving DOIs via the doi.org Handle API. Cost: a second browser profile the user has to log in to.
- **Tier 3 (institutional IP).** Licensing: remove generic tier 3, because scripted downloads from the ETH IP range are exactly the "systematic downloading" licences prohibit, traced to the institution. Allow it only where a publisher's TDM policy permits it (Springer platform at 1 req/s, Wiley TDM).
- **Tier 4 legality.** Licensing: Swiss URG Art. 24d needs *lawful access*, and (from memory, not verified) has no contract-override clause; EU DSM Art. 3 doesn't cover ETH. Draining therefore needs a per-publisher `tdm_authorisation` record, otherwise tier 4 degrades to "save the paper the human just opened", one click per work. Recommended: send the ADR to the ETH Library e-resources and TDM contact before step 7, and drop the "loosen pacing" override flag entirely (a library veto item).
- **Slice order.** raid: 2 → 4 → (3 ‖ 7) → 6 → 5, because raid holds zero TDM keys and ACS, RSC, SNMMI and T&F have no TDM API, so the browser session is its workhorse. That collides with the licensing constraint on tier 4. Resolving the order depends on the tier-4 decision above.

### Required changes to D1–D7 (no dissent)
- **D1 (data):**
  - separate `blobs(sha256 PK, rel_path)` from `artefacts`;
  - `UNIQUE(paper_id, kind, version, locator)` for multiple SI files and VoR + AM side by side;
  - paths relative to the library root;
  - identity checks in their own table with an override history;
  - partial unique indexes on `pmcid` and `osti_id`;
  - a **figure reference** (URL, caption, label) that can exist without a file (raid's digitisation queue);
  - **tables stored as structured JSON** keeping `rowspan` / `colspan` / footnotes / anchor, with CSV as a view only (raid).
- **D1 legacy (data, raid):** the TUI already collapses Abstract / Paywall / Unknown into `paywall` (`app.rs:~577`), so there is no `access_status` to preserve; map conservatively. Add a **flat-layout importer** for raid's ~109 works (`<slug>.{pdf,html,tables.json,meta.json}`, `<slug>_SI_*`), which a published datasheet cites as provenance. Record the old path.
- **D2:**
  - `acquisition_state` key `(paper_id, kind, locator)`;
  - don't store `have`: derive it from the artefacts;
  - add `pending`, `not_entitled` (logged in but 403; routes to ILL, not "log in again") and a **wanted-version** check (an AM copy of a wanted VoR stays on the action list);
  - append-only `acquisition_attempts`, plus `next_attempt_at`;
  - **work-level leases** (`acquisition_leases`, `ON CONFLICT … WHERE lease_until < now RETURNING`) so two processes never fetch the same PMC package;
  - `action_list` items carry a `drop_path`;
  - drop-ins are reconciled by an explicit `scan` / `attach`, never on read.
- **D3, the OA tier (licensing):**
  - **PMC `oa.fcgi` was retired on 2026-08-25**; use the `pmc-oa-opendata` S3 bucket;
  - gate Europe PMC on `isOpenAccess` / `inEPMC` (J Nucl Med 2022: 244 in EPMC, 74 OA);
  - **resolve metadata first, then rank** (OA VoR > AM > preprint) rather than first-win-per-kind with arXiv first;
  - landing HTML after TDM; never scrape ScienceDirect (use its Article Retrieval / Object APIs for `mmc` SI);
  - SI: DataCite `IsSupplementTo` covers T&F (Figshare); Crossref `is-supplemented-by` is ~0 for ACS and RSC, so demote it; ACS and RSC SI sit on the publisher's host and go through the pacer;
  - parse Crossref `license[]` properly (content-version, start, delay; only allow-listed CC in force counts as OA).
- **Provenance (licensing):** `license_url`, `license_content_version`, `license_start`, `publisher`, `access_basis ∈ {oa_license, tdm_licence, statutory_24d, subscription_read, ill, manual}`, `tdm_policy_ref`, `auth_context`, `retain_until`. Derived artefacts **inherit** `access_basis`.
- **Tier-4 defaults (licensing):** ≥30 s + jitter between works, ≤50 works per publisher per day, ≤150 per day total. Document that caps are per user and can't see other ETH users.
- **D5 (security):** TDM keys sent only to the key's own publisher host, with redirects disabled or checked (a 30x must not carry `X-ELS-APIKey` away). Canary non-leakage test across MCP, CLI, TUI render buffer, TRACE logs, the manifest and the SQLite file, including error paths.
- **D7 (raid):** keep **both** checks: metadata before fetch *and* served page vs OpenAlex after fetch (catches wrong-PII redirects). Store `doi_corrected_from`.
- **Content handling (security):** a content-addressed store, magic bytes checked against `format`, size caps; `read_paper` wraps full text in an **untrusted-content envelope**; never store rendered browser-tier page HTML unless it *is* the full text.
- **Migration runner (data):** `run_migrations` isn't transactional; fix it first (#250).
- **Out of scope (raid):** `citation_lib.py` / `lint_dois.py` are a repo-wide claim linter, not acquisition. Removed from the port list.

### Filed as independent bugs (found during review, exist today)
- #248: macOS keychain store passes the secret on argv
- #249: MCP `download_paper` accepts an agent-supplied `output_dir`
- #250: `run_migrations` isn't transactional or locked

### Needs an empirical spike before step 7
- `chromiumoxide` `Fetch`-domain body capture with Chrome's PDF viewer and service workers.
- Whether Shibboleth / SWITCH edu-ID SSO works in a fresh dedicated profile, and which IdP hosts the allowlist then needs.


---

# [Comment #2]() by [gerchowl]()

_Posted on September 30, 2026 at 12:17 AM_

### Decisions (maintainer, 2026-09-30)
1. **Tier 4 uses a dedicated browser profile.** scitadel launches its own Chrome profile (`--user-data-dir`), and SSO is done there once. The debugging endpoint is a local pipe (loopback only if a port is unavoidable), and each publisher has an origin allowlist. The everyday profile is never attached.
2. **No generic institutional-IP tier.** Institutional-IP fetching is allowed only where a publisher's TDM terms permit it (Springer platform at 1 req/s, Wiley TDM). No generic "try a plain request from the campus network".
3. **Tier 4 ships one-click first; bulk draining is gated.** The first cut is "save the paper I just opened": one human action per paper. Bulk queue draining needs a per-publisher `tdm_authorisation` record. Meanwhile, ask ETH Library about ACS, RSC, T&F and SNMMI.
4. **Next: write ADR-007 now.** It folds in round 1 plus these decisions: migration 013 schema, and a sketch of the Route and pacer types. Step 2 starts after #250.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 30, 2026 at 12:26 AM_

### Round 2 and outcome

- **ADR-007: #251.** The round-2 traceability and S1-implementer reviews are folded in:
  - a separate `scitadel-http` crate for `PacedClient`, which breaks the adapters↔acquire dependency cycle;
  - S1 dual-writes the legacy columns and files, so current behaviour is unchanged;
  - CHECK constraints, indexes, the lease SQL and the `tier` column on pacer grants;
  - acceptance criteria for every slice;
  - unverified legal claims marked as such.
- **Verified:** migration 013 applies on top of 001–012; the lease claim/refuse/takeover sequence, the CHECK constraints and the audit rows surviving a paper deletion (`ON DELETE SET NULL`) were tested in SQLite; the `Route` / `Pacer` / `PacedClient` type sketch compiles.
- **Slices filed:**
  - **S1** #252: model, pacer, PacedClient, importers (after #250)
  - **S2** #253: acquire, coverage, action_list, identity checks
  - **S3** #254: OA routes
  - browser pre-spike #255, then **S4** #256: one-click tier 4
  - **S5** #257: SI discovery and HTML tables
  - **S6** #258: tier-2 TDM
  - **S7** #259: gated bulk drain and TUI queue
- **Bugs found during review:** #248, #249, #250.

raid's fastest path: S1 → S2 → (S3 ∥ S4).

This issue closes when #251 merges.

---

# [Comment #4]() by [gerchowl]()

_Posted on September 30, 2026 at 12:39 AM_

Resolved: ADR-007 merged in #251 (`docs/decisions/ADR-007-2026-09-30-literature-acquisition.md`). Implementation is tracked in #252–#259, with prerequisite bugs #248, #249 and #250.

