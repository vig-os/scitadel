---
type: issue
state: open
created: 2026-11-08T12:02:14Z
updated: 2026-11-08T12:05:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/321
comments: 0
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:56.415Z
---

# [Issue 321]: [feat(hub): H7 — golden set, red/green quarantine loop and random-sample QC for extraction and embeddings](https://github.com/vig-os/scitadel/issues/321)

Part of #311. Design: ADR-008 §13 (Proposed), PR #317. Lives in `vig-os/scitadel-hub` after H0.

## Motivation

H5's extraction pipeline and H1/H2's embeddings will fail in two ways:
- **Loudly:** a crash, a QC flag, an extractor disagreement.
- **Silently:** plausible text with wrong reading order, a table with shifted columns, an embedding model that drifted.

The hub needs a test harness that grows from every real failure, and sampling that catches the silent errors. Otherwise the corpus degrades without anyone noticing, and every extractor or model upgrade is a leap of faith.

## Proposal

**Golden set.**
- A stratified fixture set of about 500–2,000 papers, covering:
  - source type: JATS / HTML / TEI / PDF;
  - publisher, field, year and language;
  - hard cases: math-heavy, table-heavy, scanned, two-column layouts, supplementary material, retracted papers;
  - **a patent stratum:** utility, design, plant and reissue patents; chemistry-heavy, many-claims and drawings-heavy patents.
- Each fixture is the raw input plus the expected tessera output, as snapshot tests (`insta`).
- **Split by licence:**
  - CC-BY/CC0 fixtures live in the repo or on HF and run in public CI;
  - all others live in a private fixture bucket and run on fleet runners only.

**Red/green growth loop.** Every extraction failure or QC flag goes through the same steps:
1. Quarantine the raw input.
2. Add a fixture with a failing test (red).
3. Fix it (green).
4. The case joins the golden set permanently.

The hub can open the issue automatically, respecting the licence split: public issues reference only CC fixtures, and private cases go to the private tracker or bucket.

**Upgrades go through the golden set first.** An extractor, model or tier-threshold change runs on the golden set and produces a diff report (what changed, per work and per block). Only after review is the corpus re-derived (new derived products; H5 provenance).

**Random-sample QC on every batch (silent errors).**
- **Ingest:**
  - a fixed random sample per batch goes through T3/T4 (H5 cascade, licence-gated) regardless of flags;
  - 300 samples with 0 errors puts the error rate below ~1% at 95% confidence (rule of three);
  - error rates are tracked per source and per extractor version.
- **Embeddings:**
  - NaN, norm and duplicate checks;
  - self-retrieval (a chunk finds itself);
  - recall@k on golden queries;
  - ANN recall against exact search (tessera#769);
  - drift between model versions on the golden set before any re-embedding;
  - the quality cost of Matryoshka truncation for Qwen3-VL-Embedding figure vectors (H5).
- **Confirmed errors become red fixtures,** feeding the loop above.
- **The same applies to the patent corpus** (H1): grant XML parsing, claims and description splitting, PatentSBERTa embeddings.

**QC metrics per work** (H5 columns): extracted-text-to-PDF ratio, empty sections, table parse rate, extractor confidence. They feed the flags that trigger escalation and quarantine.

## Acceptance

- [ ] A golden set of at least 500 papers covering every stratum above. The CC part runs in public CI and the rest on fleet runners; both are green.
- [ ] A quarantined failure becomes a red fixture through a documented (ideally automated) path; one end-to-end example is merged.
- [ ] An extractor upgrade produces a golden-set diff report before any corpus re-derivation; the report format is documented.
- [ ] Per-batch random-sample QC runs, and error rates per source and extractor version are reported per batch.
- [ ] The embedding checks run on every embedding batch; a drifted model version is blocked from re-embedding until reviewed.
- [ ] The patent corpus (H1) is covered by the same harness.

## Dependencies

H5 (pipeline, QC columns, cascade tiers), H1 (#314) and H2 (#315) (embedding batches), H4 (private fixture bucket, licence classes); #246 (System One for T2); tessera#769 (exact vs ANN recall); fleet runners for private fixtures.

