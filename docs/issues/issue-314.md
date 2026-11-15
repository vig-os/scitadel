---
type: issue
state: open
created: 2026-11-08T11:46:55Z
updated: 2026-11-11T17:24:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/314
comments: 4
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-12T08:57:26.239Z
---

# [Issue 314]: [feat(hub): H1 — USPTO patent corpus (PVGPATDIS + full text 1976+), PatentSBERTa embeddings, served walled from the hub](https://github.com/vig-os/scitadel/issues/314)

Part of #311. Design: ADR-008 (Proposed), PR #317. Lives in [nerdmachines/scitadel-hub](https://github.com/nerdmachines/scitadel-hub).

## Motivation

scitadel's PatentsView adapter has been dead since USPTO paused the PatentSearch API on 2026-03-20 (`search.patentsview.org` is NXDOMAIN; see the comment on #268). The stopgap is PatentRef, a third-party free beta (1,000 requests/month, US grants 1976+, no pre-grant publications, keyword matching without stemming). Per its own docs, PatentRef replays PatentsView data; it does no OCR and no extra disambiguation. The same data is free in bulk, and nothing on Hugging Face combines US full text 1976–present with precomputed embeddings and a search index (HUPD stops at 2018 and has no embeddings; BigPatent is 1.2M descriptions; DAPFAM is a 96.5k-family benchmark).

## Proposal

**Sources** (all free):
- PatentsView disambiguated tables: USPTO Open Data Portal bulk product `PVGPATDIS`, 35 TSV files, granted patents 1976–present (assignees, inventors, locations, CPC, citations). CC-BY-4.0. No API key for bulk.
- USPTO Patent Grant Full Text XML, weekly files, 1976–present. US government works, public domain in the US. Pre-1976 is image-only (would need OCR); out of scope.
- Optional later: pre-grant publications (application full text), EPO DOCDB.

**Pipeline** (each run writes new versioned outputs; never rewrites existing data under `/srv/anvil`):
1. Ingest TSV + XML → normalised tables (patents, claims, description sections, CPC, citations patent→patent and patent→paper, assignees, inventors) → internal Parquet staging, then sealed tessera products once tessera W0/W1 land (vig-os/tessera#778).
2. Embeddings: PatentSBERTa (768-d, MPNet) over title+abstract (and optionally first claim) as a GPU batch job via `gpu_submit` on anvil, chunked to the 30-min default runtime, `vram` declared, `priority=low`. Estimate: ~8M grants → ~12 GB at fp16 (estimate).
3. Indexes as tessera aux caches: BM25 (tessera#775), vector search (tessera#769), citation/CPC graph (tessera#776).
4. Weekly delta: new grant XML each week → a new delta product; quarterly re-seal of the base.

**Serving: walled, not published** (owner decision 2026-11-08; ADR-008 §15). The corpus, its embeddings and its indexes stay inside the hub, stored as `.tsra` only (tessera#777). Clients reach them through the hub protocol (H3 #316) and the MCP grounding path. There is no Hugging Face dataset and no other bulk release: "publish the tools, keep the data walled", so the GPU hours behind the embeddings aren't given away. The build recipe and tools are public in the hub repo, so anyone can reproduce the corpus from the USPTO bulk files themselves. Each product's provenance records its sources, licences (CC-BY-4.0 attribution for the PatentsView tables; public domain for the USPTO text), the USPTO release, and the embedding model and version. Size estimate: 100–150 GB in total, on stage-0 storage (anvil KC3000).

**Same extraction cascade and QC as papers** (ADR-008 §6, §10, §13; H5 #319, H7 #321):
- **T0** parses the USPTO full-text XML (ICE DTD) into the hub's document model (H5):
  - abstract and description;
  - claims, with the claim dependency tree as edges (claim → the claim it depends on);
  - embedded tables;
  - math and chemistry (MathML, CWU chemical structures);
  - the description of the drawings.
- **Drawings** are TIFF images. They take the figure path: stored as JPEG XL lossless (never lossy), embedded with Qwen3-VL-Embedding, and sent to the T3 vision model on sage when needed.
- **PatentsView `PVGPATDIS` TSVs** supply the disambiguated bibliographic data (assignees, inventors, locations).
- **Licence:** US patent text is public domain, so every cascade tier, including T4 (Opus via the API), is allowed. Other offices (e.g. EPO) are a later extension, each with its own licence check.
- **Out of scope:** pre-1976 patents (images only, would need OCR).
- **The golden set gets a patent stratum** (H7): utility, design, plant and reissue patents; chemistry-heavy, many-claims and drawings-heavy patents. Random-sample QC and the embedding checks run on every patent batch.
- **Layout:** base = a PatentsView release, deltas = weekly grant XML, partitioned by grant year × CPC section (H6 #320).


## Acceptance

- [ ] A reproducible build (one command, pinned inputs) produces the corpus from the USPTO bulk files.
- [ ] Row counts match the source release (grants per year reported), and a sample of 100 patents round-trips field by field against the source XML.
- [ ] Embeddings cover every grant with an abstract; the model id and version are recorded in the product's provenance.
- [ ] A client configured against the hub runs a BM25 and a vector query over the patent corpus, and the result provenance names the USPTO release and the embedding model. No corpus copy is published anywhere.
- [ ] A weekly delta run is documented and has run at least once.

## Dependencies

- H0 (repo). tessera#778 W0/W1 for sealed products (internal Parquet staging first is fine), #777 for the `.tsra`-only format, W2 for the indexes. gerchowl/g-fleet#1121 for the S3-compatible store (until then: a working copy under `/srv/anvil/projects/<project>/`). anvil disk: one 2 TB drive, so the patent corpus fits, but budget it against H2.

Also: H5 #319 (pipeline), H6 #320 (base + delta), H7 #321 (golden set, QC); tessera#779 (binary columns for drawings), #780, #781, #782.


---

# [Comment #1]() by [gerchowl]()

_Posted on November 8, 2026 at 12:21 PM_

Re-checked unverified item 10 from vm-dev (2026-11-08): `bulkdata.uspto.gov` does not resolve here either (curl: no response), while `data.uspto.gov`, `api.uspto.gov` and `developer.uspto.gov` do. This is not a network quirk on the research side: the legacy bulk host appears to be retired. Full-text grant and pre-grant XML should be pulled from the Open Data Portal's Bulk Data Directory on `data.uspto.gov` (the same place PVGPATDIS moved to). #314 should target the ODP bulk API, not bulkdata.uspto.gov.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 9, 2026 at 03:23 AM_

**Scope change (ADR-008 decision 19, 2026-11-08):** the corpus is walled. The patent corpus is built, embedded and served by the hub, **not published on Hugging Face**. Drop the HF publication tasks from this issue; the tools are published, the data is not. Cold copies go to a Hetzner Storage Box, warm copies to the g-fleet#1363 S3 box (ADR-008 §8). The only public data candidate is the tiny CC0 fixtures slice (#331, gated on open question 2).

---

# [Comment #3]() by [gerchowl]()

_Posted on November 9, 2026 at 04:55 AM_

Amended for the 2026-11-08 owner decisions (walled corpus, nerdmachines repo home, licence-gated phase 1, NC/ND excluded from paid tiers, EU DSM Art. 4 opt-out). Source: triage-matrix Cluster E; ADR-008 batch on #317.

---

# [Comment #4]() by [gerchowl]()

_Posted on November 11, 2026 at 05:24 PM_

**Candidate source: the PatentRef SQLite snapshot** (raised by PatentRef's operator on #268; checked 2026-11-11 against Zenodo record [10.5281/zenodo.23612407](https://doi.org/10.5281/zenodo.23612407) and patentref.io/LICENSES.md).

**What it is:** version 20261020.1, CC BY 4.0, three SQLite files with FTS5, about 504 GB, served over plain HTTPS from Hetzner Object Storage (Helsinki), with SHA-256 sums.

| File | Size | Contents |
|---|---|---|
| `snapshot` | 167 GB | the PatentsView tables in the API layout; 9.76M patents |
| `lapse` | 38 GB | maintenance-fee status and lapse dates |
| `text` | 300 GB | claims, brief summaries, detailed descriptions (**2005 onward only**), drawing descriptions |

**Sources:** the PatentsView bulk tables (granted to 2025-12-30), USPTO weekly grant XML from 2025-12-31 to 2026-10-20, the USPTO maintenance-fee events, and family links outside the US from Google Patents Public Datasets.

**Fit for H1.**
- **Licence:** clean for every tier, with attribution (CC BY 4.0, row by row in their licence register). The Google Patents family links carry the re-verify caveat from #323.
- **Value:**
  1. It closes the gap since PatentsView stopped: its tables end 2025-12-30, and the snapshot carries weekly grants up to 2026-10-20.
  2. The lapse and maintenance-fee catalog is new enrichment that isn't in #323's matrix (legal status matters for prior-art searchers).
  3. It's an independent oracle for this issue's "row counts match and 100 patents round-trip" acceptance.
- **Limits:**
  - It is second-hand: a third-party re-packaging of public-domain and CC BY data.
  - Detailed descriptions start in 2005, so it can't replace the 1976+ full text from the USPTO bulk XML.
  - There are no drawings and no pre-grant publications (deferred upstream).
  - SQLite is an ingest input for us, not a product format.
  - 504 GB has to fit into the stage-0 KC3000 budget (nerdmachines/scitadel-hub#12 is re-deriving the quotas).

**Recommendation:** keep the first-party USPTO bulk XML plus PatentsView as the source of truth for provenance. Add the snapshot as an **optional adapter**, used for:
- (a) bootstrapping and cross-checking the bibliographic tables;
- (b) the post-2025-12-30 delta until our own weekly-XML path runs;
- (c) the lapse catalog as enrichment.

Pin it by version and SHA-256, delete the raw file after ingest per the retention policy (it's re-fetchable), and add a row to #323.

