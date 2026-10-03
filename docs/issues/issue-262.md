---
type: issue
state: closed
created: 2026-10-02T12:22:16Z
updated: 2026-10-02T23:19:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/262
comments: 1
labels: bug, effort:medium, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:42.579Z
---

# [Issue 262]: [fix(resolve): half-parenthesised Elsevier DOIs and URL-shaped pseudo-DOIs survive shape validation and reach the fetch queue](https://github.com/vig-os/scitadel/issues/262)

## Problem

Elsevier's compact DOIs embed a balanced `(YY)` group — `10.1016/0003-2670(93)90142-7`. Any parser that treats the closing bracket as punctuation truncates them, and the truncated half **still starts with `10.`**, so it survives a naive shape check and reaches a fetch queue as if it were a DOI.

## Evidence (raid, 2026-10-02)

Nine half-parenthesised DOIs were harvested out of markdown and cx row bodies, entered an acquisition inventory, and were sent to the fetch ladder:

```
10.1016/0003-2670(93     10.1016/0016-7037(84     10.1016/s0969-8051(97
10.1016/0003-2670(77     10.1016/0022-1902(69     10.1016/s0016-7037(97
10.1016/0006-2952(73     10.1016/0022-1902(65     10.1016/s0960-894x(03
10.1016/0016-7037(70
```

(Pre-fix list; one of these was in flight and produced a live `tdm_key_missing` for a DOI that does not exist.) A further truncated token, `10.1039/qr9581200265`, is an RSC identifier with an article number and a date concatenated — a different malformation, same symptom.

The mirror-image bug is already live in raid's own linter and worth knowing about: two copies of a "compound record key" heuristic each cut a token at the **first colon** whenever the tail matched a loose regex. ACS's pre-2008 DOI form puts a colon inside the suffix (`10.1023/b:josl.0000026645.41309.d3`), so five perfectly valid speciation DOIs were truncated and reported as malformed. The rule now requires a further `:` and no `.` — the identical trap in a different character.

## Why it is worth an exported validator

Every consumer of a DOI list re-implements this and gets a different subset wrong. raid alone got it wrong **three** times in one session:

1. markdown/code-span delimiters retained on the DOI key;
2. a JSON-serialised literal `\n` (the next paragraph) retained;
3. balanced-paren truncation above.

Each one is a handful of characters of fix, and each one silently corrupts a downstream list rather than raising.

## Proposal

1. **Ship a DOI shape validator** that knows the real malformities: balanced parens (Elsevier compact), colon-in-suffix (ACS pre-2008), segmented identifiers (OUP/IOP/ChemRxiv/De Gruyter — see #261), and rejects a suffix that is a bare journal abbreviation or an ellipsis placeholder. Return a reason, not a boolean, so a caller can report *why* a candidate was rejected.
2. **Reject unbalanced brackets and never re-join** — the truncated half must not become a key. When a candidate is rejected, name it in the run output so a human can overturn the judgement; raid's version reports every rejection for exactly this reason.
3. **Apply the same validator on ingest**, not only at read time. The worst instance in this run was not a parser at all: 718 **shipped P3 records** store a ChEMBL *activity URL* (`10.6019/CHEMBL/ACTIVITY/27697493`) in `provenance.source.doi`, which scraped into the `10.` prefix and passed every shape check raid had. Validating provenance DOIs at ingest would have caught it.

## Acceptance

- [ ] A validator exported from the package returns `ok` with a reason for each rejection, and correctly accepts the real forms: Elsevier compact parens, ACS `10.1023/a:`/`b:`, OUP `10.1093/nar/…`, ChemRxiv `10.26434/…/v1`.
- [ ] The nine truncated DOIs above are all rejected, with a reason naming the unbalanced bracket.
- [ ] A ChEMBL activity URL in a DOI field is rejected at ingest.
- [ ] Consumers stop maintaining their own prefix/shape tables (see #261).

## Related

- #260 (OA routes), #261 (publisher classification and segmented identifiers).
---

# [Comment #1]() by [gerchowl]()

_Posted on October 2, 2026 at 11:19 PM_

Fixed by #272 (merged alongside #261).

`validate_doi_detailed` returns `Ok(canonical)` or a `DoiRejection` naming **why**, so a caller can report a rejection for a human to overturn rather than silently dropping the candidate.

- **Balanced parens accepted, unbalanced rejected as their own case** (`UnbalancedBracket`). All ten truncated DOIs from the raid run are in the tests, plus the stray-closing-bracket mirror. The reason text names the truncation explicitly, because the half still starts with `10.` and passes a prefix check — which is how they reached a fetch queue.
- **ChEMBL activity URL rejected** (`RepositoryUrlNotDoi`): `10.6019/CHEMBL/ACTIVITY/27697493`. This one is only detectable *with* the prefix table, since the shape is indistinguishable from `10.1093/nar/gkv1075`. A genuine repository identifier (`10.6019/chembl12345`) is asserted to still pass, so the rule is not a blanket slash ban.
- **Ingest** now validates with the detailed form and logs the reason, taking the canonical DOI from the same call.

One deviation from the issue, stated rather than silently skipped: the proposed *"reject a suffix that is a bare journal abbreviation"* rule is **not** implemented. It is not reliably distinguishable from a legitimate identifier and would be a false-positive generator. The four rules shipped are each evidenced by a real DOI from the raid run.

Also not done, and not claimed: the issue asked consumers to stop maintaining their own prefix tables. That is now possible for anything depending on `scitadel-core`, but out-of-tree consumers (e.g. raid's `tools/fetch/fulltext.py`) have to adopt it separately.

