---
type: issue
state: open
created: 2026-11-09T03:49:57Z
updated: 2026-11-09T03:51:26Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/334
comments: 0
labels: feature, effort:medium, priority:medium, semver:minor
assignees: none
milestone: 0.10.0
projects: none
parent: 246
children: none
synced: 2026-11-09T08:50:50.310Z
---

# [Issue 334]: [feat(score): abstract-pass System One relevance ranking — slice 1 of #246, client-side, no hub needed](https://github.com/vig-os/scitadel/issues/334)

## Problem

#246 describes a two-pass System One (Jev/Kev) relevance ranking plus directed reading. Its full-text "candlestick" pass depends on hub chunks (#319), but its first pass, question x abstract, does not. Today an agent calling the MCP gets search results in retrieval order, not in order of relevance to the research question, and it has no calibrated way to tell a squarely on-topic paper from a keyword match. That invites an agent to fill the gap with confident, ungrounded prose.

## Proposal

Ship pass 1 of #246 as its own slice, client-side, on the existing local DB, with no hub dependency.

- A `ScorerBackend` (non-generative, so not an `AgentBackend`) with one client for `POST /v1/systemone`. Configuration picks hosted Jev or a local Kev server; default to local when one is configured.
- One `score` question per research question, with ordered levels taken from the `get_rubric` bands. Score each candidate's abstract (title + venue fallback when there is none) and store the mean level, the per-level distribution and the confidence per `(question, paper)`.
- Rank by a documented combination of expected level and confidence; a band threshold is meaningful because the scores are calibrated.
- Expose the ranking over the MCP (and CLI) so agents receive ranked, grounded candidates, each with its calibrated probabilities, rather than an unordered list.
- Positioning: MCP agent grounding against hallucination. The agent reasons over papers the scorer has already ranked and the reader can trace back to the abstract that earned the score.
- Out of scope: the chunked full-text candlestick pass and directed reading. They wait for hub chunks (#319).

## Acceptance

- [ ] `ScorerBackend` trait and a `/v1/systemone` client, configurable for hosted Jev or local Kev, with tests against a stub server.
- [ ] Per-`(question, paper)` scores (distribution, confidence) persisted in the existing local DB via a migration; no hub needed.
- [ ] An MCP tool and CLI command returning papers ranked by score with probabilities.
- [ ] Papers without an abstract are handled by the documented fallback.
- [ ] Docs cover configuration, the privacy default (local when configured) and the ranking rule.

Refs #246, #211, #329.

