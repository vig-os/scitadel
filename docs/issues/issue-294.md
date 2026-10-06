---
type: issue
state: open
created: 2026-10-05T16:30:01Z
updated: 2026-10-05T16:30:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/294
comments: 0
labels: feature, priority:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-06T08:17:51.592Z
---

# [Issue 294]: [feat(acquire): the 'am' want has no producer — queue_add and import_flat hard-code 'vor'](https://github.com/vig-os/scitadel/issues/294)

## Problem

After #293, `artefacts.version` records what the resolver established, and a ranked author manuscript *can* file as `am` and satisfy an `am` want. So the machinery works.

But nothing ever **asks** for an `am`. Both producers hard-code `vor`:

- `acquire_queue_add` — writes `wanted_version = 'vor'`
- `import_flat` — writes `wanted_version = 'vor'`

So an `am` want can only arrive from a hand-written `acquisition_state` row or an NDJSON import whose artefact object carries an explicit `wanted_version` (#286 supports that). Nothing scitadel does produces one.

## Why it is a policy change and not a correctness fix

This is the reason it was deliberately left out of #293. Which version the user wants is a question about *intent*, and the plausible answers disagree:

- **Always `vor`** is today's behaviour and is right for a library that wants the citable article.
- **`vor` first, then `am`** is right for a reader who cannot get the VoR and would rather have the author manuscript than nothing. That is a very common real situation, and it is most of why ADR-007 has an `am` rung at all.
- **`am` when the VoR is paywalled** is the same policy expressed as a fallback rather than a second pass.

Implementing any of these changes what `acquire` asks publishers for, which is a user-facing decision with licensing implications — so it needs an explicit choice and an ADR amendment, not a default.

## Proposal

1. Decide the intent, and make it explicit: a default plus a flag, e.g. `--version vor|am|any` on `acquire`, `queue_add` and `import-flat`, with `vor` the documented default so existing libraries do not change behaviour.
2. Amend ADR-007 §1's want table to record the choice and its default.
3. If the fallback reading is adopted ("vor, then am when the VoR is unavailable"), it composes with #293 rather than replacing it: rank as now, and when the top-ranked candidate is a VoR that the ladder cannot obtain, re-rank with `am` admitted. That needs a decision about whether the *first* failed attempt counts against the pacer's daily caps — it does, and a fallback that spends two permits per work is a real budget change.

## Acceptance

- [ ] Every `acquisition_state` row scitadel writes has a `wanted_version` that came from an explicit user choice or a documented default.
- [ ] No producer hard-codes it.
- [ ] ADR-007 records the policy and its default.

Refs: #254, #253, ADR-007 §1.
