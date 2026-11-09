---
type: issue
state: open
created: 2026-11-08T12:02:07Z
updated: 2026-11-09T04:56:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/318
comments: 1
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:57.849Z
---

# [Issue 318]: [feat(hub): H4 — artefact store outside the corpus DB: raw retention, licence/entitlement gating, upload-on-acquire, detach/export](https://github.com/vig-os/scitadel/issues/318)

Part of #311. Design: ADR-008 §12 (Proposed), PR #317. The store and gating live in [nerdmachines/scitadel-hub](https://github.com/nerdmachines/scitadel-hub); upload-on-acquire and `detach` are client changes in scitadel.

## Motivation

Today every scitadel install keeps its own copy of every PDF it acquires (ADR-007). In a lab that means:
- the same paywalled PDF is fetched from the publisher once per member;
- it is stored once per laptop;
- nobody can search the full text of what colleagues already hold.

The hub should keep the **bytes once** and let each client keep **identity only** (DOI, OpenAlex ID, content hash), its annotations and a pointer to its configured hub. Licences make this more than a cache: what may be shared depends on how each artefact was obtained.

## Proposal

**An artefact store outside the corpus database.** Heavy originals sit in a content-addressed store: PDFs, plus raw JATS, TEI and HTML.
- Keys are blake3 hashes, the same identity tessera and ADR-007 use.
- It is a bucket on the fleet's S3-compatible store (versitygw, gerchowl/g-fleet#1121), not a table block inside the shard products (H5).
- `works` rows reference artefacts by hash and source URL.
- **Open access:** the PDF may be pointer-only, because it can be fetched again.
- **Paywalled:** a PDF acquired through someone's access rights is kept, as the version of record.

**Raw retention, set by size and by what fetching again costs** (ADR-008 §12):

| Raw input | Policy | Why |
|---|---|---|
| Open-access PDF | **Delete after ingest.** Keep the blake3 hash, source URL and fetch date; a re-fetch is verified against the hash. | Large (~1.5 MB each) and free to fetch again |
| Paywalled, acquired PDF | **Keep, in cold storage**, as the version of record | Hard to obtain again: needs someone's access rights |
| JATS from PMC / Europe PMC bulk | Either way; default keep (small) | Free to fetch again |
| Grobid TEI from the OpenAlex content API | **Keep, zstd-compressed.** For a 2M-paper subset that is about 40 GB raw, ~8–10 GB zstd *(estimate)*. | Fetching again costs $0.01 each |

- **Raw XML never goes into tessera products.** It lives in the cold artefact bucket only.
- **Derived products point to their raw input by hash and URL,** whether or not we hold the bytes. This needs an "external original, not held" case in tessera provenance, because tessera ADR-0033 assumes originals are kept. This is tessera#782, which adds records for external inputs tessera doesn't hold (held / cold / discarded, `--fetch-check`) and amends ADR-0033. Today a blake3-digest source reference *fails* `tessera provenance --require-complete`.
- **Why keep XML at all:**
  - re-deriving after an extractor upgrade without paying again;
  - protection against upstream link rot and retractions.

**Licence classes, recorded per artefact:**

| Class | Hub cache | Embed / index | Served in a paid tier | Phase 1 |
|---|---|---|---|---|
| CC0 / CC-BY / public domain | yes | yes | yes, with attribution | **enabled**: the only class (`public-licence-clean`) |
| CC-BY-NC / CC-BY-ND | lab-internal only | yes, internal only | **never**: excluded from every paid tier | disabled |
| Free to read, no licence (bronze/green) | lab-internal only | yes, internal only | no | disabled |
| Paywalled, obtained with a member's access rights | served only to users entitled under the same licence (e.g. ETH members); bytes kept in cold storage as the version of record | yes, internal only | never | **disabled**: licence-gated |

The corpus is walled (owner decision 2026-11-08): no class is ever published as a dataset, on Hugging Face or elsewhere.

**Phase 1 (ADR-008 decision 16, §15): designed, not enabled.** Every row and artefact carries the licence-class column, and the protocol carries it. Phase 1 writes a single class (`public-licence-clean`), and `get` / `search` can't return anything outside it. The other classes, the cold store for paywalled PDFs and the entitlement checks are designed and wired through the schema, so turning them on later is a configuration change. No code path that reads, caches or serves them is enabled until the ETH Library conversation.

The free-to-read row relies on the Swiss Copyright Act's text-and-data-mining exception for research (CopA Art. 24d). That is our reading, to be confirmed with ETH Library before H4 ships.

**Upload-on-acquire (fetch once, serve the lab).**
- When a client acquires a work through the ADR-007 pipeline, including tier-2 TDM (#258) and tier-4 browser capture (#256), it offers the artefact to the hub.
- The upload carries the licence class, the entitlement that obtained it, and the acquisition provenance.
- Other entitled clients then get it from the hub instead of the publisher.

**`scitadel detach` / `export`.**
- Pulls full copies of the papers a user tracks into local storage, for offline use or when leaving the lab.
- Checks the licence per artefact.
- Can emit a per-paper `.tsra`: that paper's rows from every H5 block plus its artefact, with provenance pointing at the hub shard's seal, so it is verifiable on its own.

**Guardrails:**
- **Never a bulk proxy for paywalled content.** The hub caches only what members fetched individually, and the ADR-007 pacer rules still apply upstream.
- **Minimal access logging.** The hub knows who read what, so it logs entitlement checks, not reading histories, and retention of those logs is bounded.

## Acceptance

- [ ] A client configured with a hub stores no PDF bytes by default. Opening a held work renders the structured document (Typst/HTML/Markdown views, ADR-008 §5) from the hub. PDF bytes are ingest inputs only and are never served to clients.
- [ ] An artefact acquired by member A is served to entitled member B without a publisher request; a non-entitled request is refused, with a test.
- [ ] Every artefact carries a licence class; publication tooling refuses anything that isn't CC-BY/CC0, with a test.
- [ ] `scitadel detach <ids|--tracked>` writes full local copies, and per-paper `.tsra` that `tessera verify` accepts.
- [ ] The raw retention policy is enforced per input class. An open-access PDF deleted after ingest is re-fetched and verified against its stored hash; a hash mismatch is flagged, never silently accepted. Tests cover both.
- [ ] Every derived product still resolves its raw input (hash + URL) when the bytes are not held.
- [ ] The ETH Library confirmation of the free-to-read classification is linked here before release.

## Dependencies

H0 (repo); H5 (the `works` table these artefacts hang off); ADR-007 acquisition (#253–#259); gerchowl/g-fleet#1121 (bucket); H3 (#316) for the client/hub protocol, which gains `fetch_artefact` and `offer_artefact` calls.


---

# [Comment #1]() by [gerchowl]()

_Posted on November 9, 2026 at 04:56 AM_

Amended for the 2026-11-08 owner decisions (walled corpus, nerdmachines repo home, licence-gated phase 1, NC/ND excluded from paid tiers, EU DSM Art. 4 opt-out). Source: triage-matrix Cluster E; ADR-008 batch on #317.

