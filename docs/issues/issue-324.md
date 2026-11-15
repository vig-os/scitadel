---
type: issue
state: open
created: 2026-11-08T12:36:56Z
updated: 2026-11-08T12:36:56Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/324
comments: 0
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: 341
synced: 2026-11-09T08:50:55.032Z
---

# [Issue 324]: [feat(hub): H12 — extraction-issue reporting: one-click per element, squelch routing by licence, correction overlay, feedback loop](https://github.com/vig-os/scitadel/issues/324)

Part of #311. Design: ADR-008 (Proposed), PR #317. Lives in scitadel (client/reader) and `vig-os/scitadel-hub` after H0.

## Motivation

The hub serves **no PDFs**. The scitadel React reader renders the structured document straight from tessera rows — elements, tables, figures, equations — so extraction quality **is** the product the reader sees. A silent misorder, a shifted table column, a dropped equation does not degrade an experience alongside the "real" PDF; there is no real PDF to compare against. The reader is the real PDF.

Readers are therefore the QC sensors the hub has that nothing else does. H7 (#321) builds the harness; H5 (#319) builds the pipeline; T2's System One checks and T3's vision-language repair catch what they can at ingest. What they cannot catch is what readers notice on the papers they actually read — a flag from a person who was going to read this element anyway, in a reader that already knows which element, which extractor version, which derivation. The information the extraction-issue report needs is already next to where the reporter is pointing.

Opening a prefilled GitHub form by hand asks the reader to re-type what the hub already knows. One click, with the fields pre-attached, is the only form of this that will see enough use to matter.

## Proposal

**One-click flow from the reader.**

- A ⚑ control appears on every element in the rendered document (paragraph, list item, table, figure caption, equation, reference link).
- Clicking it opens a lightweight reporter pinned to that element.
- Failure chips, pick-one (plus optional free-text other):
  - wrong text
  - missing text
  - wrong order
  - broken table
  - broken equation
  - figure missing / wrong
  - wrong reference link
  - other (free text)
- Optional inline correction field: the reporter types what the element **should** say. If supplied, this becomes the expected output of the red fixture (#321's red/green loop); without it, the report is still useful as a failure signal.
- A single confirm button sends it.

**Attached automatically** (reporter never types these):

- `work_id`
- extraction derivation hash
- element `ord` and `kind` (from H5's `elements` block, #319)
- anchor: the exact quote plus a short prefix and suffix, so the fixture can relocate the element even after the paper is re-derived with a different `ord` numbering
- source (JATS / HTML / TEI / PDF), `extractor@version`, and the tier verdicts that produced this element (T0..T4; H5 §cascade)
- QC metrics for this work (extracted-text-to-PDF ratio, empty-sections flag, table parse rate, extractor confidence — H5 `works` columns)
- client version, hub version, reader/renderer version (tessera#782 per-row provenance)

**Licence routing — squelch does the split.**

- CC-BY / CC0 works
  - IDs + the misextracted snippet + the reporter's correction go to the **public GitHub issue form** on the hub's issue tracker.
  - The same payload is added to the **public golden set** (#321), under the CC licence of the source.
- Unlicensed / paywalled works
  - **IDs only** reach the public path (fingerprint, failure kind, extractor version, derivation hash — no snippet, no correction).
  - The snippet + correction go via squelch's `Endpoint` route to the **hub's private quarantine** (#321) and a **private fixture** on the fleet runner.
- The **hub holds the GitHub credential server-side**; users are authenticated via **edu-ID / ORCID** before any routing decision is made.
- A client **without** a hub configured (a CLI or TUI reader, offline reading session) uses squelch's `Browser` route — GitHub's form becomes the review surface, the reporter still clicks Submit, no credential reaches the binary. The public/internal split degenerates to "nothing internal is sent", which is the correct behaviour when there is no private sink.

The routing is implemented by squelch's disclosure-class fields (gerchowl/squelch#15) and the reference `Endpoint` receiver (gerchowl/squelch#17); the hub is the operator of that receiver.

**Dedupe into votes, not duplicates.**

- Each report carries a squelch `Fingerprint` (gerchowl/squelch#16) built from `(extractor@version, failure kind, element kind, pattern components the T3/T4 verdicts name)`. Work-specific identifiers (`work_id`, element `ord`) are **not** components, by design — the whole point is grouping across papers.
- A repeat fingerprint is turned into a `+1` reaction and a comment on the existing issue, not a new issue.
- The tracker view groups by failure pattern: "87 papers, extractor grobid-0.8.3, reading-order failure above a figure in a two-column layout" is one row, not 87.

**Immediate correction overlay for the reporter.**

- On submit, the correction is also written into the reporter's **user/group layer** as an annotation of type `correction`, pinned to the element anchor.
- The reader renders the corrected text in place for the reporter (and, if a group layer, for their colleagues) the instant the report is filed. This holds whether or not the global fix has landed, and whether or not the licence class let the correction reach any public destination.
- The overlay is tied to the anchor, not to the `ord`, so it survives a re-derivation that renumbers elements.

**Global fix flow: red → fix → green → re-derive.**

1. The report becomes a red fixture (public or private per licence class) in the appropriate golden set (#321).
2. The extractor / tier / threshold change lands and turns the fixture green on the golden set, with the H7 golden-set diff report (#321).
3. The corpus is re-derived for every work matching the failure pattern (H5 derived product; the old product stays valid, #319).
4. The reporter — and everyone else with an open annotation of type `correction` on an element fixed by this pattern — is notified with the **count of papers fixed by the same pattern**. The signal "your one flag fixed 87 papers" is the retention loop the reader QC programme depends on.

**Hub as the server-side receiver.**

The hub deploys gerchowl/squelch#17's reference receiver (edu-ID / ORCID OIDC in front, server-held GitHub credential behind, per-user rate-limit, server-side `Form` re-validation against the extraction-issue form, fingerprint-based dedupe). Licence routing is the hub's additional policy layer above squelch's disclosure-class partitioning.

## Acceptance

- [ ] The reader shows a ⚑ control on every element kind listed above; clicking it opens the reporter pinned to that element, with every field in "Attached automatically" pre-filled from H5 rows + tessera#782 provenance (never typed).
- [ ] A CC-BY/CC0 report produces: a public GitHub issue (opened by the hub's server-side credential, or by the reporter's browser when no hub is configured) containing IDs + snippet + correction; **and** a public golden-set entry for the correction.
- [ ] A paywalled / unlicensed report produces: a public GitHub issue with IDs only, no snippet, no correction; **and** a private quarantine entry at the hub holding the snippet and correction; **and** a private fixture on the fleet runner. A test asserts byte-for-byte that the snippet never reaches the public GitHub issue.
- [ ] A CLI/TUI client without a hub falls back to squelch's `Browser` route: no credential in the binary, GitHub's form is the review surface, nothing internal is sent.
- [ ] Repeated reports with the same failure pattern collapse into one issue with a `+1` count, keyed by a squelch `Fingerprint` whose components do not include `work_id`.
- [ ] Submitting the correction writes a `correction` annotation to the reporter's user/group layer, pinned to the anchor; the reader renders the corrected text in place immediately regardless of whether the correction reached any public destination.
- [ ] The red → fix → green → re-derive flow is end-to-end: one real issue is filed, a red fixture lands in the appropriate golden set, the fix lands, every matching work is re-derived, and the reporter is notified with the count of papers fixed by the same pattern.
- [ ] Access logs record the entitlement check (who, when, which fingerprint), not reading histories or snippet contents — matching H4 (#318)'s minimal-logging posture.

## Dependencies

- Upstream squelch work: gerchowl/squelch#15 (typed fields with a per-field disclosure class), gerchowl/squelch#16 (caller-supplied fingerprint), gerchowl/squelch#17 (reference server-side `Endpoint` receiver with auth, rate-limit and re-validation).
- #316 (H3 client/hub protocol; this adds `submit_extraction_issue` and `fetch_my_corrections` calls).
- #319 (H5 document model and extraction pipeline; the row shape, the derivation hash, the tier verdicts, the anchor).
- #321 (H7 golden set and red/green quarantine loop; receives the red fixtures from both public and private paths and drives the re-derive).
- ADR-008 (PR #317; the design this is a sub-issue of).

