---
type: issue
state: open
created: 2026-11-08T12:02:09Z
updated: 2026-11-08T12:05:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/319
comments: 0
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:57.328Z
---

# [Issue 319]: [feat(hub): H5 — document model + tiered extraction pipeline: Arrow/Vortex blocks, tables, figures, math, chunks, embeddings, full provenance](https://github.com/vig-os/scitadel/issues/319)

Part of #311. Design: ADR-008 §9–§11 (Proposed), PR #317. Lives in `vig-os/scitadel-hub` after H0.

## Motivation

Full text is where retrieval and #246's chunk scoring pay off, but the forms it arrives in are poor storage:
- **PDF:** about 1.5 MB per paper.
- **JATS and TEI:** XML trees that can't be queried at corpus scale.
- **Typst, HTML and Markdown:** lossy re-typesettings that aren't queryable data either.

The hub stores full text as **Arrow-shaped tables in tessera/Vortex**, a document model designed for column access, compression and embedding. JATS, TEI and PDF are ingest sources, kept as raw artefacts in H4's store.

## Proposal

**Document model** (ADR-008 §9).
- **Products:** shard products of about 1–10 GB *(estimate)*, one per OpenAlex field × publication-year bucket, grouped in a collection per field under each quarterly base collection.
- **Blocks,** each sorted by `(work_id, ord)`:
  - `works`: work_id, DOI, licence class, source (jats|html|tei|pdf), extractor@version, per-work content hash, artefact hashes and source URLs;
  - `elements`: work_id, ord, parent_ord, kind, text, page, bbox[4];
  - `inline_refs`: work_id, ord, span, target kind, target id;
  - `bib`: parsed references with DOI and `matched_work_id`, the source of graph edges;
  - `table_cells`: work_id, table_id, row, col, text, `num: f64?`, unit;
  - `figures`: work_id, fig_id, caption ord, bbox, blob ref;
  - `chunks`: 300–400-token spans over elements, for #246 (derived).
- **Derived products:**
  - embeddings per model: chunk and figure vectors, row-aligned with `chunks` and `figures`;
  - figure blobs, kept in a separate product so text scans never touch images;
  - figure descriptions from the T3 vision model (chart type, axes, variables, trend), a derived text column for BM25 and text embeddings, generated lazily for the hot set only;
  - a work_id → shard index (tessera#780).
- **Numbers apart from text:** `table_cells.num`, bbox and pages are typed columns (pcodec), not strings.
- **Rendered views:** Typst, HTML and Markdown are generated on demand for the reader, GUI and LLM views; they are never stored.

**Extraction pipeline** (ADR-008 §10). Sources in order of preference per work:
1. JATS (PMC / Europe PMC);
2. publisher HTML;
3. Grobid TEI. OpenAlex holds ~71M TEI files (~1.5 TB) at $0.01 per download via the content API, so it only fills gaps;
4. our own PDF extraction (Grobid, Docling or Marker), with math OCR as GPU batch jobs on the 5090 via `gpu_submit`.

- **Tables:** from JATS first, then the extractor.
- **Figures:** cut by coordinates, captions linked. Storage is **lossless only**:

  | Source image | Stored as | Why |
  |---|---|---|
  | Vector graphics in the PDF | SVG, in a string column (zstd) | Exact and small |
  | Embedded JPEG | JPEG XL lossless transcode | ~20% smaller; the original JPEG is reconstructed bit-exactly, so its hash still verifies |
  | Lossless raster | JPEG XL lossless | |
  | 16-bit / high bit-depth | JPEG XL | WebP is 8-bit and capped at 16,383 px per side |

  - WebP/AVIF thumbnails are on-demand display views only.
  - Rust: jxl-oxide decodes; encoding goes through libjxl (C++). A pure-Rust lossless encoder is unverified.
  - Bytes go into declared media-typed binary columns (tessera#779).
- **Figure embeddings:** Qwen3-VL-Embedding (`Qwen/Qwen3-VL-Embedding-2B`, 2048-d, or the 8B at 4096-d; arXiv 2601.04720).
  - Text, images and page screenshots share one space.
  - Matryoshka truncation to 512–1024 dims, with the quality loss checked on H7's golden set.
  - The Qwen3-VL-Reranker reorders the top-k.
  - CLIP and SigLIP are weak on charts and plots.
  - Bulk embedding runs on the 5090 via `gpu_submit`; interactive work runs on sage.
- **Formulas:** LaTeX/MathML from JATS, or math OCR. Grobid's flattened formula text is a fallback only.

**Tiered extraction cascade.** Each tier runs only on what the cheaper tier couldn't settle (ADR-008 §10).

| Tier | What | Where | Escalates when |
|---|---|---|---|
| T0 | Parse JATS / publisher HTML | CPU | No structured source, or a QC flag |
| T1 | Tool extraction: Grobid, Docling, Marker, math OCR | CPU + the 5090 (`gpu_submit`) | A QC flag, or two extractors disagree |
| T2 | System One yes/no checks (Clef/Kev, #246), e.g. "is this table complete?", "is this the reading order?" | Clef hosted / Kev local | The answer is uncertain (below the calibrated threshold) |
| T3 | A Qwen vision-language model compares the page image with the extraction and repairs it. Model size to be decided; sage's specs are unverified. | sage | It can't repair, or its confidence is low |
| T4 | Opus (`claude-opus-5-5`) via the API gives the final verdict | external API | Fails → quarantine + a red fixture (H7) |

**Guardrails:**
- **Licence-gated tiers.** T4 is an external API, so only CC-BY/CC0 content may reach it. Paywalled or unlicensed content stops at T3, which runs on our own hardware, unless its licence allows more.
- **Budget caps on T3 and T4,** with queueing when the cap is reached; no silent skipping.
- **Every tier's verdict in provenance:** model and version, prompt hash, decision, confidence.
- **Escalation thresholds calibrated** against H7's golden set, and re-calibrated when a tier's model changes.

**Provenance of every process** (ADR-008 §11).
- Every derived product records, via tessera Generation / `derived_from`:
  - tool and version, and config;
  - model-weights hash;
  - input artefact hashes;
  - GPU job ID and host.
- Each `works` row records which source and extractor produced that work's rows.
- Re-extraction or re-embedding writes new derived products; originals are never touched.
- Raw inputs are referenced by hash and URL even when not held (H4 raw retention).
- tessera records provenance only per product today. Multi-step pipeline provenance, per-row provenance via a `prov_id` column, and external inputs not held are all tessera#782.

**Patents use the same pipeline** (#314):
- T0 parses the USPTO ICE XML: claims, with the claim dependency tree as edges; description; tables; MathML and CWU chemistry; drawing descriptions.
- TIFF drawings take the figure path.
- Every tier including T4 is allowed, because US patent text is public domain.

**QC metrics per work, stored as columns on `works`:**
- extracted-text-to-PDF ratio;
- empty sections;
- table parse rate;
- extractor confidence.

A work that fails or is flagged goes into H7's red/green loop.

**Extractor upgrades** run on H7's golden set first and produce a diff report. Only then is the corpus re-derived.

## Acceptance

- [ ] A sample of 1,000 works from mixed sources (JATS, TEI, PDF-only) ingests into the block schema; each work's rows reassemble into a readable Markdown/Typst view.
- [ ] Measured per-paper size, broken down by block (text, tables, figures, vectors), is compared with the original PDF and TEI. This settles the ADR's size estimates.
- [ ] Every derived product's provenance names its tool, version, weights hash, inputs and GPU job. A test checks that a product with missing provenance is rejected.
- [ ] A re-run with a newer extractor version produces a new product and leaves the old one valid.
- [ ] Chunks feed #246 pass 2; chunk embeddings are searchable via tessera#769.

## Dependencies

H7 (golden set and quarantine loop, the test harness for this pipeline); H0; H4 (artefact store for the raw sources); tessera#778 (W0 strings #628; W1 streaming #654; W2 vectors #768/#769, BM25 #775) tessera#779 (binary columns), tessera#780 (keyed references, work_id → product index), tessera#782 (pipeline / per-row / external-input provenance); #246 (consumer of chunks); anvil GPU via `gpu_submit`.

