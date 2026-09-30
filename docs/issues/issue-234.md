---
type: issue
state: open
created: 2026-09-28T23:14:14Z
updated: 2026-09-30T00:07:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/234
comments: 1
labels: feature, effort:large, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:26.150Z
---

# [Issue 234]: [feature: corpus acquisition: per-work artefact manifest (full text + SI + structured tables), Europe PMC/JATS + OSTI routes, batch acquire with coverage report and grouped action list](https://github.com/vig-os/scitadel/issues/234)

## Description

Turn scitadel from "download one file for one DOI" into **corpus acquisition**: given a list of works, get *every artefact a curator needs* (full text, supplementary files, structured tables, figures), record where each came from and under what licence, and report coverage per work with a single, grouped list of what still needs a human.

This is the complement to #230. #230 covers **how to get past a paywall**: TDM tokens, institutional IP, the human-assisted session broker, and the live TUI queue. This issue covers **everything around that ladder**:
- the unit of acquisition becomes a *work with several artefacts*, not one file
- the structured OA route (Europe PMC / PMC JATS)
- supplementary-information discovery
- provenance and licence per artefact
- batch runs over a DOI list, with a coverage report
- identity checks (DOI ↔ title)
- works without a DOI

The two issues should share one route ladder and one artefact store; #230's tiers 2–5 plug into the ladder defined here.

## Problem Statement

A dataset-curation consumer needs more than a PDF per DOI, and today every consumer rebuilds the missing parts itself.

**Measured on raid (radiometal radiopharmaceutical datasets), 2026-09-29.** The corpus is raid's primary deliverable:

| set | works | full text on disk |
|---|---:|---:|
| DOIs cited by curated records (P1 separation Kd) | 21 | 18 |
| DOIs cited by curated records (P2 stability constants) | 4 | 3 |
| DOIs cited by curated records (P3 receptor affinity, mostly ChEMBL documents) | 1,038 | 20 |
| DOIs cited by curated records (thermodynamic data) | 15 | 3 |
| candidate papers not yet curated | 594 | 47 |

- `.scitadel/papers/` holds 109 distinct works: 64 PDF, 61 HTML, 28 meta.json, plus a few xml/docx/zip/bin.
- **Supplementary information: effectively zero**, although for chemistry the numbers very often live *only* in the SI (tables of Kd vs acid concentration, titration data, binding curves).
- scitadel's DB has 8,851 paper rows, almost all metadata-only.

**What `download` does today** (`crates/scitadel-adapters/src/download.rs`): arXiv PDF → OpenAlex `best_oa_location.pdf_url` → Unpaywall → landing-page HTML, then `detect_access_status()` classifies the result as FullText / Abstract / Paywall / Unknown. That's a good core, but:

1. **One file per work.** The result is `{doi}.pdf` *or* `{doi}.html`. There's no notion of "this work has a VoR PDF, a JATS XML, 3 SI files and 4 tables, of which 2 are still missing".
2. **No structured-text route.** Europe PMC / PMC serve JATS XML for a large share of biomedical OA papers, with `<table-wrap>` tables that parse losslessly into CSV. That's far better input for extraction than PDF, and it isn't tried.
3. **No SI.** Supplementary files are linked from the publisher landing page, from JATS `<supplementary-material>`, from Crossref `relation`, and from Figshare/Zenodo/Dryad deposits. None of these are followed.
4. **No provenance/licence per artefact.** Which route produced the file, when, sha256, VoR vs accepted manuscript vs preprint, and which licence (OpenAlex `best_oa_location.license`, Unpaywall `license`, Crossref `license[]`, Europe PMC `license`). The consumer must know whether it may redistribute derived data (e.g. a CC-BY dataset release) and must cite the exact artefact version.
5. **No batch/coverage mode.** Consumers loop `download` over a list and then re-derive "what do I have / what's missing / why" themselves. raid wrote `tools/fetch/fulltext.py` (OpenAlex → DOE OSTI → TDM keys → "ask the human") and `tools/fetch/browser_cdp.py` for exactly this, and other consumers will write theirs.
6. **No identity check.** Scraped reference lists and LLM-assisted curation produce DOIs that resolve but point at the *wrong paper* (swapped or mistyped DOIs). raid had to build a DOI→title verifier (`scripts/citation_lib.py`, `lint_dois.py`) that caught fabricated and swapped DOIs repeatedly. Acquisition should refuse to file an artefact under a DOI whose resolved title doesn't match the expected one.
7. **Works without a DOI.** Old literature (e.g. 1950s–1970s radiochemistry, DOE/AEC reports) has OSTI ids, report numbers, ISBNs, or nothing. Those need a first-class identifier and an explicit `needs_ill` (interlibrary loan) state, with an exportable citation for the ILL request.

## Proposed Solution

### 1. Work → artefact manifest (data model)

Each acquired work gets a manifest, stored in the DB and mirrored as `papers/<stem>/manifest.json`:

```json
{
  "work": {"doi": "10.1021/…", "openalex_id": "W…", "pmcid": "PMC…", "osti_id": null,
           "title": "…", "year": 1998, "first_author": "…",
           "identity_check": {"expected_title": "…", "resolved_title": "…", "score": 0.97, "status": "ok|mismatch|unverified"}},
  "artefacts": [
    {"kind": "fulltext_xml", "format": "jats", "path": "…/fulltext.xml", "route": "europepmc",
     "version": "vor|am|preprint", "license": "cc-by", "license_source": "europepmc",
     "sha256": "…", "bytes": 123456, "retrieved_at": "…", "access_status": "fulltext"},
    {"kind": "fulltext_pdf", "…": "…"},
    {"kind": "si", "label": "Supporting Information S1", "format": "pdf|xlsx|csv|zip|docx",
     "source_url": "…", "route": "publisher_landing|jats|crossref_relation|figshare|zenodo|dryad", "…": "…"},
    {"kind": "table", "label": "Table 2", "format": "csv", "derived_from": "fulltext_xml",
     "locator": "table-wrap id=T2", "…": "…"},
    {"kind": "figure", "label": "Figure 3", "format": "png", "derived_from": "fulltext_xml", "…": "…"}
  ],
  "missing": [
    {"kind": "si", "reason": "needs_login", "publisher": "ACS", "hint_url": "…"},
    {"kind": "fulltext_pdf", "reason": "tdm_key_missing", "publisher": "Elsevier"}
  ]
}
```

- **Artefact kinds:** `fulltext_pdf`, `fulltext_html`, `fulltext_xml`, `si`, `table`, `figure`.
- **Per-artefact status vocabulary** (shared with #230): `have`, `oa_fetchable`, `tdm_available`, `tdm_key_missing`, `needs_login`, `needs_ill`, `unavailable`, `identity_mismatch`.

### 2. Route ladder additions (OA tier, before #230's tiers 2–5)

1. arXiv (existing)
2. **Europe PMC REST** (`/search?query=DOI:…` → `fullTextXML` for OA + author-manuscript content; `supplementaryFiles` endpoint). Also PMC OA (`oa.fcgi`) for the tar package, which includes SI.
3. OpenAlex `best_oa_location` / all `locations[]` (existing, extend to all OA locations, not just best)
4. Unpaywall (existing)
5. **DOE OSTI** (`osti.gov/api/v1/records?doi=…` or by OSTI id) for national-lab reports. raid's `fulltext.py` has this route and it should move upstream.
6. Landing-page HTML (existing), now also **harvesting SI links** from the page (publisher-specific selectors for ACS, Elsevier, Springer, Wiley, RSC, T&F, SNMMI, with a generic `supplementary|supporting information|ESI` fallback).
7. **Crossref `relation`** (`has-supplement`, `is-supplemented-by`) and **DataCite** lookups for Figshare / Zenodo / Dryad SI deposits.
8. → #230 tiers (TDM, institutional IP, human session, `needs_manual`).

Pacing rules from #230 (per-publisher rate limit and daily cap, on by default, hard to disable) apply to every route that touches a publisher host, including SI harvesting.

### 3. Structured tables (derived artefacts, structured sources only)

- From JATS `<table-wrap>` and publisher HTML `<table>`: write each table as CSV plus its caption/footnotes, keeping the table's locator (id/label).
- **No PDF table parsing in scitadel.** That's the consumer's extraction problem (docling/MinerU/etc.). scitadel only derives what the structured source already contains, losslessly.

### 4. Batch acquisition + coverage report

CLI:
```
scitadel acquire --from dois.txt|refs.bib|works.ndjson [--artefacts fulltext,si,tables] [--dry-run] [--resume]
scitadel coverage --from … [--format md|json|csv]
```
MCP:
- `acquire(works[], artefacts?)` → job id, resumable and idempotent (re-running skips `have`, retries transient failures only)
- `acquire_status(job_id)`
- `coverage(works[] | search_id)` → per-work × per-artefact status matrix plus totals
- `action_list(works[] | job_id)` → **one grouped list of what needs a human**, grouped by route/publisher: "log in to ACS (65 works: 40 SI, 25 full text)", "Elsevier TDM key missing (41)", "ILL (3 works, citations attached)". This is the list a curator hands to a person, merged with #230's `auth_pending()`.

Input rows may carry `expected_title/year/first_author` so the identity check runs at acquisition time.

### 5. Identity check

For every work with an expected title: resolve (OpenAlex → Crossref → DataCite) and fuzzy-match title (plus year and first author as tie-breakers). On a mismatch, don't file artefacts under that DOI: set `identity_mismatch` and report both titles. This reuses what `bib_verify` / `bib_snapshot` already do for .bib files, generalised to any acquisition input.

### 6. Works without a DOI

The identifier is `{doi | pmcid | osti_id | isbn | report_number | url}`. Works with no retrievable route get `needs_ill` and an exportable citation (BibTeX / RIS / plain text) for an interlibrary-loan request. When the human drops the file into the work's directory, the next `acquire --resume` picks it up and records `route: manual`.

### 7. Licence surfacing for downstream redistribution

`coverage --format json` includes per-artefact `license` and `version`, so a consumer can decide mechanically whether derived data may ship (e.g. CC-BY release) or needs a TDM / "facts only" basis. scitadel reports; it doesn't decide.

## Alternatives Considered

- **Keep building this in each consumer** (status quo: raid's `tools/fetch/fulltext.py` + `browser_cdp.py` + `citation_lib.py`). Every consumer re-implements routes, SI selectors, licence parsing and pacing, each slightly wrong. Pacing mistakes can get an institution's access suspended (see #230 Pitfalls), so this belongs in one well-tested place.
- **Use Zotero + plugins.** Good for a human's library; not scriptable/agent-drivable at batch scale, no per-artefact licence/provenance model, no SI manifest.
- **PDF-only plus downstream parsing.** Loses the lossless JATS tables and the SI, which for chemistry is where most numeric data lives.

## Additional Context

- **Consumer:** raid (`github.com/gerchowl/raid`), whose first priority as of 2026-09-28 is a publication-acquisition campaign: an inventory of every source and candidate paper with per-artefact status, fetching everything reachable without a human, then one grouped user-action list, then curation with blind re-extraction per paper. raid will retire `tools/fetch/fulltext.py` once scitadel covers routes 2–7.
- **raid pacing baseline for publisher hosts:** ≥20 s per paper, one tab, queued DOIs only, never crawl, never evade rate limits.
- **Related:** #230 (paywall access ladder, session broker, TUI queue), #112 (download state persisted), #50 (institutional-access hint).
- **Code touchpoints:**
  - `crates/scitadel-adapters/src/download.rs` (route ladder, `AccessStatus`, `file_stem_for`)
  - `crates/scitadel-core` (Paper model → work + artefacts)
  - `crates/scitadel-mcp/src/tools.rs` (`download_paper_tool`, `read_paper_tool` should prefer `fulltext_xml` > `fulltext_html` > `fulltext_pdf`)
  - `bib_verify` / `bib_snapshot` (identity check)

### Suggested scope / phases

**P0: manifest + structured OA + batch**
- [ ] Work/artefact data model and `manifest.json`; migrate existing single-file downloads into it (`kind` inferred, `route: legacy`)
- [ ] Europe PMC JATS + PMC OA package route; OSTI route
- [ ] `acquire` / `coverage` CLI + MCP, idempotent and resumable, `--dry-run`
- [ ] Identity check on acquisition input; `identity_mismatch` status
- [ ] Per-artefact sha256, route, retrieved_at, version, license (+ license_source)

**P1: SI + tables + action list**
- [ ] SI harvesting from JATS, landing pages (publisher selectors + generic fallback), Crossref relations, DataCite (Figshare/Zenodo/Dryad)
- [ ] JATS/HTML tables → CSV with caption and locator
- [ ] `action_list` grouped by route/publisher, merged with #230 `auth_pending()`
- [ ] Non-DOI identifiers + `needs_ill` + citation export; manual drop-in pickup

**P2: polish**
- [ ] TUI coverage view (per-work artefact matrix) alongside #230's queue view
- [ ] `read_paper` prefers structured text; figures extracted from JATS packages

### Acceptance

- Running `scitadel acquire` over raid's 594 candidate DOIs (input with expected titles) produces a manifest per work and a coverage report, with:
  - every work in exactly one status per artefact kind
  - zero artefacts filed under a mismatched identity
  - a re-run that makes no network calls for `have` artefacts
- For a JATS-available work, the tables in the report equal the `<table-wrap>` count and each round-trips to CSV.
- `action_list` output is a single grouped list whose counts sum to the `missing` totals in `coverage`.
- Rate limiting and per-publisher caps apply to SI harvesting as well as full text (tested).

## Impact

- **Who benefits:** any consumer that builds datasets from literature (raid today). It removes per-consumer fetch code and makes coverage/licence auditable.
- **Compatibility:** additive. `download` keeps working and becomes "acquire one work, full-text artefacts only". The DB needs a migration for the artefact table. The manifest mirror is new files next to existing ones.
- **Risk:** SI harvesting touches publisher hosts more often. It must go through the same pacing and caps as #230, and never crawl beyond the landing page of a queued work.

## Changelog Category

Added

---

# [Comment #1]() by [gerchowl]()

_Posted on September 30, 2026 at 12:07 AM_

Step 1 (the joint design for #230 + #234) is spiked in #247: a work/artefact data model, one status vocabulary, the route ladder, a persistent pacer, and the credential and browser-session (CDP) isolation boundary. Nothing gets implemented until #247's decisions are settled.

