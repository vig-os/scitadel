---
type: issue
state: open
created: 2026-09-29T22:49:12Z
updated: 2026-09-29T22:51:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/246
comments: 2
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:24.138Z
---

# [Issue 246]: [feature: System One (Jev/Kev) relevance ranking — abstract pass, chunked full-text 'candlestick' pass, then directed LLM reading](https://github.com/vig-os/scitadel/issues/246)

## Summary

Use a System One decision model (TypeSafe **Jev**, hosted, or its open-weights counterpart **Kev**, local) to rank relevance in two cheap, non-generative passes. Only then hand a large LLM a **reading map**: which papers, and which passages inside them, are worth its context.

1. **Pass 1 — question × abstract.** Score every candidate paper's abstract against the research question. This gives a calibrated distribution over the rubric bands, not generated prose.
2. **Pass 2 — question × full-text chunks.** For the pass-1 shortlist, split the full text into small chunks and score each one. Aggregate the chunk scores per paper into a **candlestick**: where relevance peaks, how spread out it is, and how much of the paper is relevant.
3. **Pass 3 — directed reading.** Give the generative backend the top chunks with their locations (section, page, offsets) instead of whole papers. `reasoning` is generated only for what survives, and it is grounded in the passages that earned the score.

This is a concrete instance of #211, which argued the shape (non-generative scorer for ordering, generation only for the shortlist) but stopped at abstracts and rerankers. It adds the full-text stage and names the model family.

## Why chunk instead of reaching for a bigger context model

Chunking isn't a workaround here; it's what these models are good at:

- **Kev** was trained on inputs of **at most 384 tokens**. Its server accepts up to 65,536 tokens (plus 8,192 per question), but its README says accuracy drops on long documents. On questions whose answer is buried in 1k–6k tokens of other text, Kev-9B scores **0.556** and Kev-27B **0.833**.
- **Jev** allows 64k tokens per request, with a 32k budget for the input text. Many papers fit, but one score per paper collapses *where* the relevant part is, and that location is the most useful thing pass 3 could get.
- An abstract (~150–300 tokens) sits right inside Kev's trained length, so pass 1 plays to its strength.
- Chunks of ~300–400 tokens keep pass 2 there too. The cost is O(chunks), and it's cheap: Jev is $0.04 per million input tokens with no output charge, at 70–500 ms per call; Kev-4B takes ~300 ms for five questions on a 32 GB Mac. Scoring is also embarrassingly parallel, and one Kev forward pass over a chunk can answer several questions at once.

A larger-context *generative* model only enters at pass 3. By then it reads a few kilobytes of pre-selected passages rather than whole PDFs.

## Proposal

### Scorer backend

- Add a `ScorerBackend` (the interface question #211 raised). A non-generative scorer can't produce `reasoning`, so it shouldn't pretend to be an `AgentBackend`.
- One client for **`POST /v1/systemone`**. Kev implements TypeSafe's API contract, so the same code talks to:
  - hosted **Jev** (TypeSafe or OpenRouter; the data leaves the machine), or
  - a local **Kev** server (0.8B / 4B / 9B / 27B, Apache-2.0; nothing leaves the network).

  Configuration picks the endpoint. Abstracts are public, but research questions and annotations may not be, so default to local when one is configured.
- Question shape: one **`score`** question whose ordered levels are the rubric bands from `get_rubric`. The response is a mean level, a per-level distribution and a confidence. Optionally add `noul` sub-questions ("does this passage report a measurement of X?") so the rubric can be decomposed per research question.

### Pass 1 — abstract ranking

- Score every paper in a search against its question; store the distribution and confidence per `(question, paper)`.
- Keep the top-k, or everything above a band threshold, for pass 2. Because the scores are calibrated, a threshold means something, which a reranker's scores don't allow.
- No abstract? Fall back to title + venue, or push the paper straight to pass 2 when full text exists.

### Pass 2 — chunked full text and the candlestick

- Chunk the extracted full text. Reuse the `pdftotext -layout` path in `crates/scitadel-mcp/src/extract.rs`; JATS or HTML from #234 once available. Chunk on section and paragraph boundaries, ~300–400 tokens with a small overlap. Keep the section heading, page and character offsets on every chunk.
- Score each chunk with the same question; store per-chunk results (new table, e.g. `chunk_scores(question_id, paper_id, chunk_id, section, page, start, end, score, probs, confidence)`).
- Aggregate per paper into a **relevance candlestick**:
  - **high** = the best chunk, i.e. the paper contains *something* squarely on-question;
  - **body** = the interquartile range of chunk scores (tall: uneven; short: uniform);
  - **low** = the floor;
  - **mass** = the fraction of chunks above a band, which separates a core paper from a single paragraph mentioning the topic;
  - plus a **position strip** showing where along the paper the relevant chunks sit (e.g. all in the methods, or all in the SI tables).

  Rank by a documented combination (peak × confidence first, mass as the tiebreak). Keep the raw candle so the TUI and agents can show it rather than a single opaque number.

### Pass 3 — directed reading

- `prepare_assessment` / `prepare_batch_assessments` gain a mode that returns, per paper, the **top-N chunks with their locations** instead of the whole text. The generative backend writes `reasoning` from those passages and cites them.
- The top chunks can become **annotations** anchored at their offsets (#49's anchoring). The human reader opens the paper at the passages that drove the score.
- MCP: expose the candle and chunk hits (e.g. `get_relevance_map(question_id, paper_id)`), so an agent can ask "what should I read?" before `read_paper`.

## Evaluation (before building the full pipeline)

This is cheaply falsifiable with data we already have:

- **Pass 1:** saved assessments (`get_assessments`) give a reference ordering. Compare Kev/Jev abstract scores with Spearman, nDCG@k and band accuracy, as #211 proposed.
- **Pass 2:** existing **annotations** mark where humans and agents found the relevant text. Measure whether the top-scored chunks overlap annotated passages (hit@N by chunk).
- Compare Kev sizes (0.8B / 4B / 9B / 27B) and hosted Jev on both. If the smallest local model matches, the default is obvious; if none does, close with the numbers.

## Open questions

- Should the pass-2 cutoff be top-k or a calibrated band threshold? Make it configurable, but which default?
- The candle aggregation rule. Evaluate a few candidates against the annotation hit set rather than picking one a priori.
- Does per-domain fine-tuning pay off? Kev ships a fine-tune path starting from the released checkpoints, and saved assessments are natural training labels. That's a follow-up, not v1.
- Caching: chunk scores are keyed by `(question, chunk text hash, model version)`, so re-running after a question edit only rescores what changed.

## Acceptance criteria

- [ ] A `ScorerBackend` with a `/v1/systemone` client, configurable for hosted Jev or local Kev, with a test double for CI (no network).
- [ ] Pass 1 scores and stores every `(question, paper)` abstract; the ranking is visible in the question dashboard (#133) and via MCP.
- [ ] Pass 2 chunks full text with location metadata, scores the chunks, and stores both chunk scores and the per-paper candle.
- [ ] Pass 3: `prepare_assessment` can hand the generative backend the top-N located chunks instead of the full text.
- [ ] An evaluation report on existing assessments and annotations (pass 1: Spearman / nDCG; pass 2: chunk hit@N) for at least one Kev size, committed with the PR.

Refs: #211 (non-generative scoring), #234 (full-text / JATS artefacts feeding pass 2), #230 (access ladder), #49 (annotations as anchors and eval signal), #133 (question dashboard), #60 (`AgentBackend`)

---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 10:50 PM_

### Which models are actually self-hostable (checked 2026-09-30)

- **TypeSafe Jev** is closed: hosted API only, no public weights. It has the large window (64k per request, 32k budget for the input text).
- The **Apache-2.0, self-hostable** options are independent reimplementations of the same design, not Jev itself:
  - **Kev** (Jared Palmer; 0.8B–27B) serves the same `/v1/systemone` API. It accepts up to 65k tokens but was trained on inputs of at most 384 tokens.
  - **autotrust/JEV-27B** and **JEV-9B** (AutoTrust AI; unaffiliated with TypeSafe despite the name). JEV-27B truncates inputs over **1,024 tokens** by default, needs about 79 GB on a B200-class GPU, and serves an OpenAI-compatible endpoint, not `/v1/systemone`.
  - OpenJev, Laya and others follow the same pattern.

### Large context vs. chunking: evaluate both

A large window is tempting: one call per paper, and no chunking or aggregation code. So pass 2 gets two modes, and the evaluation decides the default rather than me:

- **whole-paper**: one Jev call per full text (hosted only, since only Jev has the window);
- **chunked candlestick**: local Kev (or Jev) per ~300–400-token chunk.

Compare the two on the same shortlist:
- ranking agreement with saved assessments;
- chunk hit@N against annotations. Whole-paper mode has no location signal, so it can't score this at all, and that trade-off should be visible in the report;
- cost and latency.

If whole-paper Jev ranks as well as the chunked mode, it's the simpler default wherever sending text to a hosted API is acceptable. Chunking stays the local, private option, and the only one that can tell pass 3 *where* to read.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 29, 2026 at 10:51 PM_

### Candidate: Laya (Convai, Apache-2.0)

A strong fit for **pass 1** and the chunk scoring in **pass 2**, with one caveat that shapes how we'd ask it.

- **Drop-in:** `laya-serve` exposes the same `POST /v1/systemone` API as Jev and Kev, so the one scorer client covers it. It also ships a Python library, ONNX Runtime support and an MCP server.
- **Size and speed:** a ModernBERT encoder, not an LLM. The English model is 421M parameters with a 512-token context; the multilingual one is 322M with 1,024 tokens (8,192 in long-document mode). It takes ~33 ms per question on a GPU and ~0.2–0.5 s on a CPU, or 100–330 questions/s batched on a T4. An abstract, or a 300–400-token chunk, fits its native window, and it runs on hardware with no big GPU.
- **Caveats** (from its own model card):
  - ordinal `score` is its **weakest question type** (0.372 on SST-5), and our rubric-band question is exactly that;
  - the base checkpoints are **near chance zero-shot** on typed decisions; the fine-tuned checkpoint reaches 0.766;
  - it ships **over-confident** and needs temperature recalibration;
  - `noul` answers "can follow its option labels instead of the state".

**Implication for the question design:** don't lean only on a 5-band `score`. Evaluate alternative phrasings per model:
- a `noul` question ("Is this passage relevant to <question>?") using P(yes) as a continuous score;
- a coarse 3-level `choice`;
- the band `score`.

Recalibrate the temperature on saved assessments before thresholding. Saved assessments are also natural fine-tuning labels if zero-shot falls short, and both Laya and Kev ship fine-tune paths.

The candidate grid is now **Laya-EN**, **Kev** (0.8B / 4B / 9B), and hosted **Jev** (whole paper, and chunked). If Laya matches Kev on pass 1, CPU-only triage of every candidate becomes the obvious default.

