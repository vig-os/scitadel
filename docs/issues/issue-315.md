---
type: issue
state: open
created: 2026-11-08T11:46:56Z
updated: 2026-11-08T11:49:58Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/315
comments: 0
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:58.687Z
---

# [Issue 315]: [feat(hub): H2 — OpenAlex lab mirror: snapshot subset, publication-date subscriptions, rolling refresh on the free key](https://github.com/vig-os/scitadel/issues/315)

Part of #311. Design: ADR-008 (Proposed), PR #317. Lives in `vig-os/scitadel-hub` after H0.

## Motivation

A lab asks OpenAlex the same questions over and over, through each member's scitadel, on each member's $1/day key. A shared local copy answers from local data and frees the budget. OpenAlex's model is "free data, paid services": the quarterly snapshot is free (CC0, ~770 GB of Parquet, ~626M records), but the "changed since" sync filters (`from_updated_date`, `from_created_date`) and the daily snapshot are Member+ only ($10k/year). The free key can still keep the parts a lab cares about fresh.

## Proposal

**Base:** the quarterly public snapshot (`s3://openalex/data/parquet/`, no sign-in), filtered to a **subset** of works: by topic/field/subfield, by institution (the lab's own), by source, or by the citation neighbourhood of the lab's libraries. Estimate: 5–50 GB per subset. A full mirror is **not** a goal: works alone are hundreds of GB, and sealed in tessera before #628 is fixed an estimated ~2.5 TB, which does not fit anvil's 2 TB drive.

**Daily jobs on one free key ($1/day = 10,000 list calls):**
1. **Subscriptions.** Per topic/author/institution/source subscription, a list query with `from_publication_date` = last run minus a lookback window (default 90 days), `per_page=100`, cursor paging. Even every new work in OpenAlex is an estimated few hundred calls a day (~$0.05); a realistic lab subscription set costs under $0.01.
2. **Rolling refresh.** Re-fetch known works in batches of 100 IDs per list call (`openalex_id:W1|…|W100`), hottest first (works in members' libraries, recently read, recently cited). The remaining budget refreshes an estimated ~950k works/day.
3. **Single-entity GETs** (free) for on-demand freshness when a client opens a paper.
4. **Merges and deletions:** detected when a refresh returns a different ID or nothing; reconciled quarterly from the snapshot's `merged_ids` and `deleted_ids` files.

Each daily run writes a new delta product; each quarterly snapshot writes a new base. "Current" is a pointer, never an in-place rewrite.

**Known gaps (accepted):** a work indexed late with a publication date older than the lookback window is missed until the next quarterly snapshot; changes to works outside the subset are not tracked.

**Optional paid path:** if an institution holds Member+ (or Member at $5k/year, $20/day), the hub uses `from_updated_date` sync instead of the subscription heuristics. Same storage model; only the delta source changes. Worth asking ETH Library whether ETH is already a member (unverified).

**Embeddings:** reuse existing CC0 sets before computing our own (`colonelwatch/abstracts-search`, MiniLM 384-d; `GlobalCampus/openalex-multilingual-embeddings`; schemas unverified). Compute only for works they miss, as GPU batch jobs.

## Acceptance

- [ ] A configured subset is built from the snapshot without downloading the full dump, reproducibly.
- [ ] Subscriptions and rolling refresh run daily and log credits used (`X-RateLimit-Credits-Used`); one week of runs stays under $1/day on one key.
- [ ] Merged and deleted IDs are reconciled; a test covers a merged work.
- [ ] Freshness report: for each subscription, the newest publication date held and the age of the oldest unrefreshed hot work.
- [ ] The OpenAlex API key never appears in logs or error strings (see #276).

## Dependencies

- H0 (repo). tessera#778 W0 (especially #628) and W1 for sealed products and bounded-memory queries; internal Parquet staging first is fine (never published). gerchowl/g-fleet#1121 for storage. Note an OpenAlex `per_page` bug in scitadel's adapter (asks for 200; the documented max is 100), being fixed in a separate PR.

