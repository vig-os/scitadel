---
type: issue
state: closed
created: 2026-10-02T23:20:06Z
updated: 2026-10-03T08:28:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/275
comments: 1
labels: bug, effort:small, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-04T07:34:05.878Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on October 3, 2026 at 08:28 AM_

Fixed by #279. S2's identity check is no longer blocked.

## The defect was subtler than the issue described

The issue said `doi.org` had no pacing bucket. It had one — **named** `doi.org`, because that is its registrable domain. So any check on the *bucket name* reported it as handled, while it silently carried the unknown-host **policy**: 5 s minimum interval, 500 requests. 1,889 DOIs = 2 h 37 m of resolution before a single PDF is fetched.

My first routing test asserted the bucket name and **passed with the entry removed**. It now compares policies, which is what was actually wrong.

## The numbers are sourced, not invented

doi.org's resolution documentation publishes **no** rate limit for the proxy, and a probe of 30 sequential resolutions (~1.9 req/s) returned no 429 and **no rate-limit headers at all**.

The only published figure naming doi.org is DataCite's, for requests arriving via doi.org content negotiation: *1000 requests per 5 minutes per IP* (~3.3 req/s). Registered interval is 500 ms — 2 req/s, under an inferred rather than observed ceiling, cited in the table as such.

The 1,000-per-24 h cap is **our policy choice**, labelled as ours. A real campaign will raise it, and the correct response is caching, not a bigger unsourced number.

## Still open

Item 4 of the issue — **resolution caching** — is not implemented. It belongs with S2's identity check, which is what would call it, so it moves there rather than landing ahead of its caller. doi.org's proxies cache handle values with a 24 h TTL, so an unchanged DOI never needs re-resolving.

Three tests, each confirmed to fail with the entry removed.

