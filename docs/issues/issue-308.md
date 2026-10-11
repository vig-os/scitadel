---
type: issue
state: open
created: 2026-10-10T12:38:44Z
updated: 2026-10-10T12:38:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/308
comments: 0
labels: refactor, effort:medium, priority:low
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-11T08:00:31.275Z
---

# [Issue 308]: [refactor(adapters): CandidatePaper.r#abstract is still a plain String, so the write side stops at the model boundary](https://github.com/vig-os/scitadel/issues/308)

## Problem

#306 made `Paper.r#abstract` and `Paper.full_text` `UntrustedBody`, so every
publisher-controlled field on the model is enveloped. `CandidatePaper` — the
pre-dedup transport in `scitadel-adapters` — still carries a plain `String`
abstract, produced by every metadata adapter (arxiv, epo, inspire, lens, openalex,
patentsview, pubmed) and consumed by dedup and the write paths.

So the envelope stops at the model boundary. A publisher's abstract travels as
`String` from the adapter that parsed it to the row that stores it, and only
becomes typed when it is read back out.

That is deliberate — it is #306's boundary, and it matches where #303's title
envelope stopped — but it is a gap, and it is the gap a future change would fall
through.

## Why it is a follow-up and not a bug

Nothing is reachable differently today: the abstract becomes `UntrustedBody` on
every read path, and `Paper::new`/`row_to_paper` are where the transition happens.
The risk is structural, not live — a new write path added against `CandidatePaper`
would inherit `String`.

## Proposal

Make `CandidatePaper.r#abstract` an `UntrustedBody` at the point each adapter
builds it. Each adapter already parses the field out of a document, so each one
gains one call to `UntrustedBody::publisher_supplied(...)` — the same shape as the
seven data-path call sites #306 added comments to.

Expect the blast radius to be the adapter suite (`scitadel-adapters`: arxiv, epo,
inspire, lens, openalex, patentsview, pubmed) plus dedup and the write paths.

## Acceptance

- [ ] No adapter produces a plain `String` abstract.
- [ ] The transition to `UntrustedBody` happens once, at parse time, not
      per-consumer.
- [ ] Existing probe and contract tests still pass with no fixture change.

## Note

This is the same class as #291's audit: a column or field that no writer touches.
The migration-013 audit now catches the SQL shape; nothing catches the *type*
shape on the write side. If this lands, a `CandidatePaper` field walk would be
worth folding into that same audit.

Refs: #287, #306
