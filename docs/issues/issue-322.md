---
type: issue
state: open
created: 2026-11-08T12:08:15Z
updated: 2026-11-08T12:08:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/322
comments: 0
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:55.928Z
---

# [Issue 322]: [feat(hub): H8 — worldwide patent corpus: DOCDB/INPADOC families, multilingual full text, claims, patent↔paper bridge](https://github.com/vig-os/scitadel/issues/322)

Part of #311. Design: ADR-008 (Proposed), PR #317. Lives in `vig-os/scitadel-hub` after H0.

## Motivation

H1 (#314) is US-only: USPTO grants and PatentsView tables, 1976+. Worldwide filings are roughly 3.5M applications a year (approximate; see WIPO IP Statistics for current numbers), of which roughly 1.6M go to China's office and about 600k to the USPTO. Much prior art (CN/JP/KR/DE, utility models) has no US family member, and a US-only corpus silently misses it.

## Proposal

**Sources to evaluate** (the licence-and-cost triage is a sibling issue; reference "source triage spike (sibling)"):

- **EPO DOCDB** — worldwide bibliographic data covering 100+ patent offices; the family backbone.
- **INPADOC** — patent families and legal status.
- **EP full text** — European Patent Office full text for text analytics.
- **OPS API** — EPO Open Patent Services, free up to 4 GB/week. scitadel's existing `crates/scitadel-adapters/src/epo.rs` is already an OPS adapter (OAuth2 client-credentials against `ops.epo.org/3.2`, published-data search endpoint).
- **Google Patents Public Datasets on BigQuery** — worldwide full text; `google_patents_research.publications` carries `embedding_v1` (64-d), `top_terms` and `similar` documents, usable as a QC baseline (schema to verify in the triage spike).
- **WIPO PATENTSCOPE** — WIPO's own international patent database.

**Design**:

- **Family-level dedupe on DOCDB families** so one invention is one hit, however many offices it was filed in.
- **Multilingual embeddings** (e.g. Qwen3-Embedding) so an English query finds CN/JP/KR text without a bulk machine-translation step; translate on demand only for display.
- **Claims embedded per independent claim**, with the claim dependency tree as graph edges (same shape as H1).
- **Patent ↔ paper bridge** via non-patent-literature citations: the Marx & Fuegi "Reliance on Science" dataset links NPL citations to OpenAlex/MAG ids (licence to verify in the triage spike).
- **CPC predicted for papers**: train on patent text → CPC, extend CPC assignment to the paper corpus.
- **Chemistry structures**: SureChEMBL SMILES (licence to verify) with molecular fingerprints for substructure search over patent chemistry.
- **Inventor / assignee ↔ OpenAlex author / institution linking.**
- **Drawings** via Qwen3-VL-Embedding, same figure path as H1.

**Same extraction cascade and QC as the rest of the hub** (ADR-008 §6, §10, §13; H5 #319, H7 #321):

- **T0** structured parse (office XMLs, DOCDB/INPADOC bibliographic) into the hub's document model (#319).
- **Tools** and **System One** tiers for enrichment.
- **Qwen VL on sage** for figures.
- **Opus (T4)** only where the licence permits, per office and per source.
- Random-sample QC on every batch; the golden set (#321) gets a worldwide patent stratum: CN/JP/KR/DE grants, utility models, chemistry-heavy, many-claims and drawings-heavy patents.

**Storage** as tessera `.tsra` (Vortex): shard products with a base + delta lifecycle (tessera#781), per-row and per-pipeline provenance (tessera#782).

## Acceptance

- [ ] Source triage spike (sibling) decides which of OPS, BigQuery, PATENTSCOPE and DOCDB bulk the hub ingests, with licences and costs on the record.
- [ ] A reproducible build (one command, pinned inputs) produces a worldwide patent corpus for the chosen sources.
- [ ] DOCDB family dedupe collapses a sample set of multi-office families to one hit, verified against INPADOC family lists.
- [ ] Multilingual embeddings cover every family's representative text; an English query returns the expected CN/JP/KR hits on a small eval set.
- [ ] A patent ↔ paper bridge query (from a known paper's NPL citations) returns the expected patent families, and the reverse direction too.
- [ ] The golden set (#321) has a worldwide patent stratum; random-sample QC runs on each batch.
- [ ] Dataset card records sources, licences and provenance per source.

## Dependencies

- #314 (H1 US patents) — this extends the pattern established there.
- ADR-008 (PR #317).
- #318 (H4 artefact store), #319 (H5 document model + extraction cascade), #320 (H6 base + delta lifecycle), #321 (H7 golden set and QC).
- tessera#778 (W0/W1/W2/W3); #781 (keyed delta products), #782 (provenance).
- Source triage spike (sibling) — licences and costs settled before ingest.

