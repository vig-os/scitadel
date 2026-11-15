---
type: issue
state: open
created: 2026-11-08T11:45:56Z
updated: 2026-11-08T12:05:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/311
comments: 0
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: none
children: 313, 314, 315, 316, 318, 319, 320, 321, 322, 323, 324, 325, 326, 327, 328, 329, 330, 331, 332, 333
synced: 2026-11-09T08:51:00.082Z
---

# [Issue 311]: [feature: scitadel-hub — lab corpus server (OpenAlex + patent corpus on tessera, local BM25/vector/graph retrieval)](https://github.com/vig-os/scitadel/issues/311)

## Summary

A lab-level **corpus server**, `scitadel-hub`, that holds a local copy of the literature (OpenAlex) and patent (USPTO) corpora and serves search and retrieval to every scitadel client in a lab: lexical (BM25), semantic (vectors) and citation-graph search over local data, followed by System One relevance scoring (#246). It lives in a separate repo, `vig-os/scitadel-hub`, and stores its data as sealed tessera products.

The design is **ADR-008** (PR #317; status Proposed). This epic tracks the work in scitadel; the storage-format work is tracked in tessera.

## Why

- **Patents have no working source.** USPTO paused the PatentsView PatentSearch API on 2026-03-20, and `search.patentsview.org` no longer resolves (reported on #268, verified 2026-11-08). USPTO's Open Data Portal API serves application metadata, not full-text patent search. The bulk data is still free.
- **Keeping up with OpenAlex is the paid part.** The free key is a $1/day budget (list call $0.0001, search call $0.001, single-entity GET free). The "changed since" sync filters and the daily snapshot are Member+ ($10k/year). The quarterly snapshot is free, CC0, ~770 GB of Parquet.
- **A lab repeats the same queries.** Every member's scitadel hits the same APIs for the same topics. A shared cache answers from local data and turns search from rate-limited API calls into local queries.
- **The hardware is next to it.** anvil's RTX 5090 (via `gpu_submit`) can embed a whole corpus in a batch job; System One models (Clef/Kev, #246) can score thousands of candidates per query.

## Work

### In scitadel (sub-issues of this epic)
- [ ] #313 H0 — split off `vig-os/scitadel-hub` once ADR-008 is accepted
- [ ] #314 H1 — patent corpus build + Hugging Face publication
- [ ] #315 H2 — OpenAlex lab mirror (snapshot subset, subscriptions, rolling refresh)
- [ ] #316 H3 — `hub` source adapter + client/hub protocol in scitadel
- [ ] #318 H4 — artefact store outside the corpus DB: raw retention, licence/entitlement gating, upload-on-acquire, `detach`/`export`
- [ ] #319 H5 — document model + tiered extraction pipeline (JATS/HTML/TEI/PDF → Arrow/Vortex blocks; T0–T4 cascade; lossless figures; Qwen3-VL figure embeddings; full provenance)
- [ ] #320 H6 — base + delta lifecycle: tombstones, newest-wins reads, compaction, retention
- [ ] #321 H7 — golden set, red/green quarantine loop, random-sample QC (papers and patents)

### Upstream
- **tessera** — vig-os/tessera#778 (epic):
  - W0: #758 (vortex fork pin) and #628 (string-heavy tables ~3× Parquet; full OpenAlex works estimated at ~2.5 TB sealed vs ~770 GB Parquet);
  - W1: #654 / #666 / #668 (streaming DataFusion provider, multi-product scans);
  - W2: #768 → #769 (vector columns, vector search), #775 (BM25 cache), #777 (publish `.tsra` on HF: dataset card + HTTP range reads of columns via the ADR-0034 object-store reader);
  - W3: #776 (graph edges + CSR cache).
  - Gap issues filed for the hub: #779 (media-typed binary columns for figure bytes), #780 (declared keys / keyed references, work_id → product index; W1), #781 (keyed delta products; W2), #782 (pipeline + per-row provenance, external inputs not held; W1).
  - The caches depend on ADR-0054 (tessera#365, still Proposed).
- **g-fleet** — gerchowl/g-fleet#1121: S3-compatible research-data store (versitygw) plus read-only POSIX for GPU jobs. Capacity: anvil's only documented drive is a 2 TB 990 PRO.

### Stopgap already in flight
- The PatentsView adapter moves to the PatentRef-compatible endpoint (#268 thread). Best-effort only: a third-party free beta.

## Sequencing

1. ADR-008 accepted → H0 (repo split).
2. tessera W0 + W1 land → H1 and H2 write sealed products and query them in bounded memory.
3. tessera W2 → BM25 + vector retrieval in the hub; H3 exposes it to clients.
4. tessera W3 → citation and patent graph features.
5. H7's golden set starts with the first ingest batch of H1, H2 or H5, before any corpus-wide extraction. Every extractor or model change is diffed on it first.
6. H4 (artefact store) and H5 (document model) unlock full text in the hub; H6 (base + delta) is needed once the first deltas arrive.

H1 and H2 can stage ingest internally as Parquet before tessera W0/W1 land and re-seal into `.tsra` afterwards (the staging is never published); everything the hub holds is derived and rebuildable.

## Acceptance (epic)

- A scitadel client configured with a hub answers a patent search and an OpenAlex topic search from local data, falls back to the live APIs when the hub misses, and reports which one answered.
- The patent corpus is published on Hugging Face (`.tsra` only, with a dataset card stating licences and provenance; readable column-by-column straight from HF).
- The OpenAlex mirror stays within the free $1/day API budget, measured over one week.

## Related
#246 (System One relevance ranking), #268 (contract tests; PatentsView retirement), ADR-007 (acquisition: the hub does not crawl publishers; clients acquire through ADR-007 and offer artefacts to the hub, H4).


