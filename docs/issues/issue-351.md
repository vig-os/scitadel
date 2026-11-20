---
type: issue
state: open
created: 2026-11-19T10:54:39Z
updated: 2026-11-19T10:54:45Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/351
comments: 1
labels: feature, phase:design
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:23:18.604Z
---

# [Issue 351]: [feat: systematic-review mode — protocol, per-source searches, screening with evidence, extraction table, RoB, computed PRISMA 2020](https://github.com/vig-os/scitadel/issues/351)

Context: owner request 2026-11-19. AnswerThis (YC F25) launched an end-to-end systematic-review flow (LinkedIn, 2026-11):

1. One question becomes eligibility criteria and search strings for PubMed, ClinicalTrials.gov, OpenAlex, Semantic Scholar and arXiv.
2. Every record is screened, with a reason and evidence shown.
3. Values are extracted from the full text with page references.
4. Then risk of bias, meta-analysis, a manuscript draft, and a live PRISMA 2020 diagram.

A human can overrule each decision, and a supervisor can review.

scitadel already has most of the parts: federated search, screening, rubric assessments (`assess_paper`), annotations, citation chasing, OA acquisition and the reader. What's missing is the **frame** that ties them into one auditable review.

## Positioning (from scitadel-cloud `docs/competition.md`)

Systematic-review screening is **parity, not an advantage**: Covidence, Rayyan, DistillerSR and ASReview are all there. A PRISMA flow alone won't win. The differentiators have to be:
- **Local corpus:** the review runs against the lab's scitadel-hub mirror (scitadel-hub#82). Searches are fast and reproducible, and there's no re-fetching.
- **Structured reader:** evidence anchors point to document blocks in the structured model, not just a page number.
- **MCP-native:** an agent drives the whole pipeline. The human gates every step.
- **Provenance:** every count in the PRISMA diagram can be traced back to rows.

## Scope

1. **Review protocol object:** question (PICO/PECO), eligibility criteria as typed rows, sources, search strings per source (generated, then edited), and dates. It is versioned, and it can be exported as a PROSPERO-style registration draft.
2. **Search runs per source,** recorded with the exact query, date, hub snapshot/generation and hit count. Deduplication runs across sources, with every merge shown. This depends on cross-source identity, scitadel-hub S-003 (#26).
3. **Screening stages** (title/abstract, then full text) as decision rows: include, exclude or unsure, with a reason from the criteria list and an evidence anchor (a passage or block id). Each row also records the decider: model, human, or human overruling a model.
   - Dual screening, with conflicts resolved by a third reviewer.
   - Cheap triage could use the System One tier (kit#158 pattern); a lean or uncertain answer escalates to a human.
4. **Extraction table:** a per-review schema (fields, units). Each cell has an evidence anchor and a verified/unverified state.
5. **Risk of bias:** RoB 2 / ROBINS-I as rubric templates over the existing assessment machinery.
6. **PRISMA 2020 flow,** computed from the rows and never typed in. It is exported as SVG/PDF.
7. **Exports:** RIS/BibTeX of included studies, CSV/Parquet of the extraction table, and the screening log.

**Out of scope for v1:** meta-analysis statistics and manuscript drafting. They come later, and only if interviews show a pull for them.

## Acceptance

- [ ] a protocol → search → dedup → two screening stages → extraction → PRISMA run on a real small question, end to end through the CLI and MCP
- [ ] every PRISMA number reproduces from the stored rows; re-running against the same hub generation gives the same counts
- [ ] a human can overrule every model decision, and the overrule is recorded as its own row
- [ ] measured against ASReview on the same question and records: time to finish screening, and recall on the human-labelled includes

Related: scitadel-hub#82 (local sources), scitadel-hub#26 (identity), kit#158 (s1 decision models), scitadel-cloud competitor note (AnswerThis)

---

# [Comment #1]() by [gerchowl]()

_Posted on November 19, 2026 at 10:54 AM_

Competitor note: nerdmachines/scitadel-cloud#6 (AnswerThis).

