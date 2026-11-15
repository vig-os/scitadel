---
type: issue
state: open
created: 2026-11-09T03:22:09Z
updated: 2026-11-09T03:22:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/332
comments: 0
labels: feature, effort:medium, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:51.338Z
---

# [Issue 332]: [feat(hub): H20 — grounding benchmark: citation existence, support and licence correctness](https://github.com/vig-os/scitadel/issues/332)

Part of #311. Design: ADR-008 (PR #317), positioning (Context). Lives in scitadel-hub.

## Motivation
The agent-grounding claim needs evidence: how often a grounded answer cites something real, relevant and correctly attributed, compared with ungrounded generation.

## Scope
- A fixed, versioned question set across fields (biomed, materials, physics, CS) plus patent prior-art questions.
- Metrics:
  - citation-existence rate (resolves to a real DOI or patent number);
  - support rate (the cited passage supports the claim);
  - attribution accuracy;
  - licence-class correctness.
- A harness that runs the scitadel MCP and baselines reproducibly (pinned models, seeds and corpus release).
- Whether head-to-head results against named third-party products are published is an open owner decision. The harness supports both.

## Acceptance
- One command reproduces the numbers from a pinned corpus release.
- Results carry full provenance (§11).

## Depends on
H17, H5 (#319), H7 (#321).

