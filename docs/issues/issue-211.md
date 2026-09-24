---
type: issue
state: open
created: 2026-09-23T16:43:44Z
updated: 2026-09-23T16:43:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/211
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-24T11:57:14.628Z
---

# [Issue 211]: [idea: non-generative relevance scoring — calibrated/reranker scores for ordering, generation only for the shortlist](https://github.com/vig-os/scitadel/issues/211)

## Idea

Relevance assessment is currently done by *generating* it. `get_rubric` asks a
model to:

> Score on a scale of 0.0 to 1.0 — 0.0-0.2 not relevant … 0.8-1.0 highly relevant
> **"Respond with valid JSON only: `{"score": float, "reasoning": "string"}`"**

…and every `AgentBackend` from #60 (Claude Code, Gemini CLI, `llm`, Ollama,
Anthropic REST) is a **generative** model. That works, but it is the wrong shape
for the scoring half, in three measurable ways:

1. **"Respond with valid JSON only" is a request, not a guarantee.** Schema
   validity is probabilistic. A non-generative scorer makes an off-schema or
   out-of-range value structurally impossible.
2. **`reasoning` is generated for every paper in a batch**, including the ones
   that score 0.1 and are never read. `prepare_batch_assessments` pays full
   generation cost across the whole search.
3. **The scores aren't calibrated.** The rubric makes *absolute* claims
   ("0.6-0.8 = directly addresses aspects"), but an LLM's `0.73` is not a
   probability and isn't stable across papers, sessions, or backends. You can
   sort by it; you can't reliably threshold on it.

There is also a concrete failure mode we hit while benchmarking the fleet's
local backends: with thinking enabled and a modest `max_tokens`, llama.cpp
returned **256/256 reasoning tokens and an empty `content`** — a well-reasoned
nothing. Batch assessment is exactly that call shape.

## This is textbook reranking

`(research question, paper)` → relevance is the canonical cross-encoder
reranker task — far more squarely so than most uses of it. That category is
mature and enormous:

| model | downloads | likes |
|---|---|---|
| `BAAI/bge-reranker-v2-m3` | 17,267,935 | 1211 |
| `Qwen/Qwen3-Reranker-0.6B` | 1,299,433 | 400 |
| `jinaai/jina-reranker-v3` | 818,752 | 145 |
| `mixedbread-ai/mxbai-rerank-base-v2` | 29,625 | 64 |

## …but reranking alone doesn't satisfy the rubric

Worth being precise, because the two are often conflated:

|  | cross-encoder reranker | calibrated decision model (Jev-shape) |
|---|---|---|
| answers | one fixed question (relevance) | arbitrary typed questions |
| output | one scalar | typed value + distribution + confidence |
| score means | **relative order within a list** | **calibrated probability** |

A reranker will order papers within one search beautifully. It will **not** give
you "this is a 0.85, i.e. a core paper" in a way that's comparable to last
month's search — and the rubric's five bands are exactly that absolute claim.
So: reranker for ordering, calibrated scorer if the bands are load-bearing.

Jev (TypeSafe's System One model) is the right *shape* for the bands — Score
against ordered levels, returning a distribution plus confidence — but it has
**no open weights** (`hf models list --author typesafe` is empty), so it means a
hosted API. Community "open-jev" reimplementations exist but are days-old
third-party fine-tunes with ~1.3k downloads against the rerankers' 17M, and
without the RLCD calibration that is the actual claim.

## Proposal

Add a **non-generative scorer** as an `AgentBackend` (or a sibling trait — it
cannot satisfy the existing one, see below), and make assessment a two-stage
pipeline:

1. **Score every candidate** with the local scorer — cheap, schema-safe, no prose.
2. **Generate `reasoning` only for the top-k** you actually read, via the
   existing generative backends.

This is strictly less work than today: you stop paying generation cost for
papers you discard, and the ranking gets a purpose-built model.

**Design note:** a non-generative scorer does *not* fit `AgentBackend` cleanly —
it can't produce `reasoning`, which the rubric requires. That's a real interface
question (separate `ScorerBackend` trait? optional `reasoning: Option<String>`?)
and probably the first thing to decide.

Local builds exist for both fleet boxes, so nothing need leave the network:
- sage (MLX): `vserifsaglam/Qwen3-Reranker-4B-4bit-MLX`, `kerncore/Qwen3-Reranker-0.6B-MLX-4bit`
- anvil (GGUF): `dean2155/Qwen3-Reranker-0.6B-Q8_0-GGUF`, `prithivMLmods/Qwen3-Reranker-4B-F32-GGUF`

Privacy is a weaker constraint here than for the mail index — abstracts are
public — but research *questions* and annotations are not necessarily, and
running locally makes the question moot.

## We already have the eval set

Unlike most "try a different model" proposals, this one is cheaply falsifiable:
`get_assessments` holds saved human/LLM assessments. Before building anything,
run a candidate reranker over papers that already have scores and compare
ranking agreement (Spearman / nDCG against the saved order, plus band accuracy).

If a 0.6B reranker matches the current backend's ordering, the case is made on
cost and determinism alone. If it doesn't, the issue closes with a number
attached.

Refs #25 (current LLM relevance scoring), #56 (`get_rubric`), #60 (`AgentBackend`).

