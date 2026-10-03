---
type: issue
state: open
created: 2026-10-02T23:20:06Z
updated: 2026-10-02T23:20:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/275
comments: 0
labels: bug, effort:small, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:41.511Z
---

# [Issue 275]: [fix(acquire): doi.org has no pacing bucket, so S2's pre-fetch identity check will resolve every DOI at 5s/request](https://github.com/vig-os/scitadel/issues/275)

## Problem

\`scitadel_http::BucketPolicyTable\` (added in #273) deliberately does **not** route \`doi.org\`. An unrouted host falls to the "unknown host" default from ADR-007 §4: **5 s minimum interval, 500 requests per rolling 24 h**.

That default is fine for an incidental third-party host. It is wrong for \`doi.org\`, because S2's pre-fetch identity check resolves **every DOI in the campaign through it** (ADR-007 §3). At 5 s/request, a 1,889-DOI corpus — the size in #260's evidence — is 2h37m of pure DOI resolution before a single PDF is fetched.

It was left unrouted rather than given an invented limit: nothing documents a Handle-API rate limit, so a number would have been made up.

## Why this blocks S2 rather than being cosmetic

The identity check is a gate — a work is not fetched until its identity is confirmed. So this is on the critical path of every acquisition, not a background nicety.

## Proposal

1. **Measure before deciding.** Run the resolution load a real campaign implies and record the observed behaviour: does \`doi.org\` rate-limit a polite client at all, and at what request rate? The Handle system documentation and any published limits are the primary source; measurement is the fallback.
2. **Register the measured bucket.** If \`doi.org\` is effectively unmetered for single-DOI resolution, give it a small minimum interval (a second or two) with a generous cap, and document the evidence next to the entry like the other rows in the table.
3. **Do not make it a metadata bucket.** \`PaceTier::Meta\` semantics ("the API's documented limit, no extra floor") are right for OpenAlex/Crossref, but \`doi.org\` is a resolution service on the critical path of every work, so it deserves its own row rather than inheriting a floor meant for bulk metadata queries.
4. **Cache negative and positive resolutions.** If measurement shows rate limiting is real, the mitigation is not only pacing: resolved DOIs should be reusable across campaigns, and a work whose DOI is unchanged from a previous run should skip resolution entirely.

## Acceptance

- [ ] \`doi.org\` appears in the bucket policy table with a limit backed by a citation or a measurement, not an assumption.
- [ ] A campaign of realistic size does not spend hours in DOI resolution.
- [ ] The table entry explains what the number is and where it came from.

## Related

#252 (S1 — this table shipped in #273), #253 (S2 — the identity check that depends on it).
