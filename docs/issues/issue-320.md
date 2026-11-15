---
type: issue
state: open
created: 2026-11-08T12:02:12Z
updated: 2026-11-08T12:05:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/320
comments: 0
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:56.875Z
---

# [Issue 320]: [feat(hub): H6 — base + delta lifecycle: tombstones, newest-wins reads, compaction, retention (OpenAlex + patents)](https://github.com/vig-os/scitadel/issues/320)

Part of #311. Design: ADR-008 §2 (Proposed), PR #317. Lives in `vig-os/scitadel-hub` after H0.

## Motivation

H2 (OpenAlex) and H1 (patents) both produce a stream of releases: a quarterly snapshot or bulk release, plus daily or weekly changes. The hub needs one lifecycle that does four things:
- keeps every release sealed and add-only;
- answers queries against "current" without rewriting anything;
- folds changes into a new base;
- frees disk on a 2 TB drive under an explicit policy, not by accident.

## Proposal

**Layout.**
- `base/<release>/`: a collection per quarterly snapshot (OpenAlex) or PatentsView release (patents). It holds the H5 shard products and their derived products.
- `delta/<date>.tsra`: the same blocks, holding upserted rows plus **tombstones** (`id`, reason: `merged → W…` | `deleted`), with small BM25 and vector caches of its own.
- `current`: a tiny, content-addressed collection `{ base, deltas[] }`. It is the only thing that moves.
- **Patents:** base = a PatentsView release; deltas = weekly USPTO grant XML, almost all additions. Partitioned by grant year × CPC section (A–H, Y).
- **OpenAlex:** partitioned by field × publication-year bucket (H5); deltas come from H2's daily jobs.

**Reads.**
- The logical table is the base plus the deltas, with the newest version of each ID winning and tombstones dropping or redirecting IDs.
- In DataFusion this is a union plus an anti-join on the (small) delta ID set.
- Searches run against the base and delta caches, merge the top-k, and mask superseded rows.

**Compaction.**
- When a new quarterly release arrives: build the new base, retire the deltas older than its snapshot date, and repoint `current`.
- Optionally, merge each month's deltas into one product along the way.
- Every compaction output is a new sealed product.

**Retention.**
- Hub data lives in its own versitygw bucket (g-fleet#1121), with an explicit policy. Default: keep the last 2 bases and their deltas.
- The fleet's add-only rule for `/srv/anvil` covers research data written by GPU jobs. The hub's working copy is a rebuildable mirror of the bucket, and expiring a generation follows the bucket's retention policy, never an ad-hoc delete.
- Reconciling the two rules is part of this issue, agreed in g-fleet#1121.

**Size blocker.** tessera#628: string-heavy tables seal about 3× larger than Parquet. The architect triaged it as a format decision (a determinism test on ksb, then an ADR). Until it lands, sealed bases are estimated at about 3× the size of their Parquet equivalent.

## Acceptance

- [ ] A base plus three deltas (including a merge and a deletion) reads as one logical table with newest-wins and tombstones applied; this is tested.
- [ ] A hybrid search over base plus deltas never returns a superseded or tombstoned row; this is tested.
- [ ] Compaction produces a new base whose logical content equals base plus deltas, and `current` moves atomically.
- [ ] The retention policy is written down, enforced by the bucket or by a job, and agreed in g-fleet#1121.
- [ ] The same mechanism serves OpenAlex (daily deltas) and patents (weekly deltas).

## Dependencies

H0; H5 (block schema); H1 (#314) and H2 (#315) as producers; tessera#781 (keyed delta products: upsert / tombstone / redirect, newest-wins read, compaction, retention), tessera#780 (keyed references); tessera ADR-0033/0049/0064 (collections, numbered leaves); gerchowl/g-fleet#1121.

