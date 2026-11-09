# ADR-008 — scitadel-hub: a lab corpus server (repo split, tessera storage, sync strategy, retrieval)

**Status**: Proposed.
**Date**: 2026-11-08
**Supersedes**: —
**Context**: #311 (epic), #313–#316 (H0–H3), #318–#321 (H4–H7), #268 (PatentsView retirement), #246 (System One ranking), vig-os/tessera#778, gerchowl/g-fleet#1121, gerchowl/g-fleet#1363

Figures marked *(estimate)* are back-of-envelope numbers, not measurements.
Prices and limits for third-party services are as published on 2026-11-08.

## Context

scitadel queries live APIs for every search: PubMed, arXiv, OpenAlex,
INSPIRE-HEP, EPO, PatentsView, Lens and the acquisition sources of
ADR-007. Four things changed:

1. **Patent search lost its free source.** USPTO paused the PatentsView
   PatentSearch API on 2026-03-20. `search.patentsview.org` no longer
   resolves (NXDOMAIN, checked 2026-11-08), and the key page
   redirects to `data.uspto.gov/support/transition-guide/patentsview`.
   The Open Data Portal (ODP) API that replaces it serves application
   metadata and file wrappers, not full-text patent search; USPTO says
   search will return, with no date. A third party, PatentRef, answers
   the old PatentSearch grammar (free beta: US grants 1976+, no
   pre-grant publications, 1,000 requests/month), and per its docs
   replays the PatentsView data without further processing (reported on
   #268). The bulk data behind all of this is still free:
   - the PatentsView disambiguated tables, now ODP bulk product
     `PVGPATDIS` (35 TSV files, granted patents 1976–present,
     CC-BY-4.0);
   - USPTO Patent Grant Full Text XML (weekly, 1976–present, public
     domain in the US). Pre-1976 is image-only and would need OCR.
2. **OpenAlex prices freshness, not data.** Since February 2026 a free
   key carries a $1/day budget: a filtered list call costs $0.0001, a
   search call $0.001, and a single-entity GET is free. `per_page` is
   capped at 100. The filters that return "everything changed since a
   date" (`from_updated_date`, `from_created_date` and their `to_`
   pairs) and the daily snapshot are **Member+** ($10k/year). The
   quarterly snapshot is free: CC0, on a public S3 bucket, in JSONL and
   (since June 2026) Parquet, ~770 GB compressed, ~626M records.
3. **Labs repeat queries.** Several members of one lab search the same
   topics through their own scitadel, each against the live APIs and
   each on their own key and rate limits. Nothing is shared.
4. **Local compute is next to the data.** anvil has an RTX 5090,
   reachable as queued batch jobs (`gpu_submit`), and System One
   models (Clef hosted, Kev local) can score thousands of candidates
   cheaply (#246). Both pay off only if candidates come from a local
   corpus instead of paginated API calls.

ADR-007 settled how scitadel *acquires* full text for a curated set of
works. This ADR settles where a lab keeps a *searchable copy of the
corpus* and how clients use it. The hub does not crawl publishers: it
holds metadata, abstracts, patent full text (public domain), the
derived indexes, and the full text that **clients acquire** through
ADR-007 (§12), stored once per lab instead of once per laptop and
gated by licence.

**Positioning.** The hub is a stepping stone toward a scitadel MCP
that grounds AI agents against hallucination, with per-row provenance
and licence class first-class in every protocol response. The target
is institutional knowledge in the database and graph, exposed through
a wire protocol a third party can implement. The architectural
decisions below — the walled corpus, per-row licence columns, the
`capabilities` and `coverage` advertisements, the protocol crate under
a permissive licence — fall out of that target.

## Decision

1. **Four repos** (§1). `vig-os/scitadel` stays open (`MIT OR
   Apache-2.0`) as the client: CLI, MCP, TUI, GUI, Tauri + web + mobile
   app, source adapters, acquisition (ADR-007), review workflow,
   local SQLite, and `scitadel-hub-proto`. `vig-os/scitadel-hub` is
   public and dual-licensed (§16). `vig-os/scitadel-hub-deploy` is
   **PRIVATE** and carries the NixOS module, the OCI image build and
   the Helm chart; on-prem comes via the commercial grant or a support
   contract. `vig-os/scitadel-cloud` is private and comes later. The
   fleet (`gerchowl/g-fleet`) consumes the module from the private
   deploy repo pinned by tag and never patches it locally.
2. **The seam is a source adapter.** scitadel gains a `hub` source
   that is tried before the live APIs for the sources a hub declares it
   covers, and falls through when it does not. The wire types live in
   one small protocol crate; domain types come from `scitadel-core`.
3. **Storage is tessera.** Every corpus snapshot and every delta is a
   sealed, content-addressed tessera product (`.tsra`, tables in
   Vortex). Storage is add-only: a new release writes new products,
   and "current" is a pointer.
4. **Queries run on DataFusion**, through tessera's provider, not
   DuckDB.
5. **Retrieval indexes are tessera derived caches** (ADR-0054 in
   tessera): BM25 (tantivy), vector search, and a citation/CPC graph
   as edge tables plus an adjacency cache. **No graph database** is
   the source of truth.
6. **OpenAlex sync stays on the free key:** a quarterly snapshot
   subset, publication-date subscriptions, and a rolling ID-batch
   refresh. The Member+ sync path is optional, for institutions that
   have it.
7. **Patents are built from USPTO bulk data**, embedded on anvil's GPU,
   and **served by the hub as walled corpus** (§6). They are **not
   published on Hugging Face** — the corpus is walled; the tools are
   published. PatentRef remains a stopgap in the client adapter, not
   a dependency of the hub.
8. **The hub runs on staged infrastructure** (§8). Stage 0 is anvil
   (versitygw + Hetzner Storage Box cold tier via restic; GPU through
   `gpu_submit`; tenant simulation on microVMs via `microvm.nix`).
   Stage 1 moves serving to a Hetzner AX box plus Hetzner Object
   Storage. Stage 2 adds NixOS k3s/microVMs for hosted org and
   personal spaces, plus hourly-rented Hetzner GPU for backfill. On-prem
   runs the NixOS module / OCI / Helm from the private deploy repo.
9. **Full text is stored as a document model, not as documents** (§9).
   Papers become Arrow-shaped tables in tessera/Vortex shard products,
   one block per element type, linked by keys. JATS, TEI, HTML and
   PDF are ingest sources; Typst, HTML and Markdown are rendered views.
10. **Extraction runs as a tiered cascade** (§10): each tier, from
    parsing up to a frontier model, sees only what the cheaper tier
    could not settle, and the tiers are gated by licence.
11. **Every process leaves full provenance** (§11): tool, version,
    config, weights, inputs, job and every cascade verdict.
12. **Heavy originals stay out of the database** (§12). PDFs and raw
    XML live in a content-addressed artefact store. A raw-retention
    policy by size and by the cost of fetching again decides what is
    kept, and licence classes gate who may read what. Clients keep
    identity and annotations; the hub keeps bytes; `detach` brings full
    copies back to a client.
13. **Quality is a growing test suite** (§13): a golden set, a red/green
    quarantine loop, and random-sample QC on every batch, for papers and
    patents alike.
14. **Serving runs on CPU; the GPU is for ingest** (§14). Extraction
    tiers T1 and T3, bulk embedding, figure descriptions and
    re-embedding after a model upgrade go to the 5090 through
    `gpu_submit`; filters, BM25, ANN search, graph hops and fusion
    serve from CPU and NVMe. Query embedding is client-side by
    default; the hub advertises the exact embedding model and rejects
    mismatched query vectors.
15. **Build first on a strictly licence-clean corpus** (§15). Phase 1
    ingests only content that is CC0, CC-BY or public-domain per
    item, and whose access terms allow bulk harvest. The corpus is
    **walled** — ingested, embedded, indexed and served by the hub;
    not published on Hugging Face. Within the licence-clean envelope
    phase 1 goes for **maximum depth** (full text, figures, tables,
    cross-links, embeddings for as much of the corpus as we can fit),
    not minimum viable. The sources are listed in §15.
16. **Licence-gated features are designed but disabled in phase 1.**
    The entitlement classes and per-class gating of H4 (#318), local
    display of licensed content for entitled lab users, and COUNTER
    5.1 / SUSHI reporting for publisher usage, are designed and
    wired through the schema and the protocol so later turn-on is a
    configuration change — but no code path that reads, caches or
    serves non-CC0/CC-BY content is enabled in phase 1. `get` and
    `search` cannot return anything outside the licence-clean set.
17. **The ETH Library approach comes after the demo.** A Research
    Collection harvest agreement (ETH's institutional repository,
    which today rate-limits off-network bulk callers), the lab's
    reading of CopA Art. 24d for internal mining of free-to-read
    works, a publisher local-hosting addendum negotiated through the
    Consortium of Swiss Academic Libraries, and COUNTER 5.1 / SUSHI
    reporting are deferred until a working phase-1 demo exists. We
    approach ETH Library with a running hub, not a slide deck.
18. **Licensing** (§16; not legal advice). The scitadel client stays
    `MIT OR Apache-2.0`. The hub is dual-licensed
    `AGPL-3.0-only OR LicenseRef-scitadel-Commercial`, with written
    AGPL §7 additional permissions for client/cloud interop. The
    protocol crate `scitadel-hub-proto` stays in the scitadel repo
    under `MIT OR Apache-2.0` (an AGPL proto crate would contaminate
    clients and integrators; this resolves the old open question 1).
    The protocol spec is Community Specification License 1.0;
    conformance vectors are CC0-1.0.
19. **The corpus is walled; the tools are published.** The processed
    corpus and the GPU hours that produce it are not given away: a
    curated public dataset is no longer a moat, and bulk copies would
    only dilute the running service. The one possible exception is a
    tiny CC0 conformance/fixtures slice (`scitadel-hub-fixtures`),
    held open as an open question. tessera#777 (tsra on HF) is
    reframed to optional/selective rather than the main publication
    path.
20. **The client app is Tauri 2 desktop and mobile plus a web dual
    build** (§17), sharing a Rust backend through typed transport
    (`specta` / `tauri-specta`). Offline-first on a personal
    machine is free and local; hosted personal, lab and organisation
    spaces are paid tiers reached over the web.
21. **A user may detach full copies of THEIR tracked works**
    (ADR-007 + §12). Bulk export of the processed corpus isn't
    offered — it is the wall.

## 1 · Repo split and deploy topology

| | scitadel (client) | scitadel-hub (server) |
|---|---|---|
| Contents | CLI, MCP, TUI, GUI; source adapters; acquisition (ADR-007); review workflow; local SQLite | Ingest (snapshot, bulk files, deltas); sync jobs; index builds; query service |
| Dependencies | Light: reqwest, rusqlite, ratatui, … | Heavy: tessera, Vortex, DataFusion, tantivy, an embedding runtime |
| Runs on | A laptop | anvil |
| CI / release | Fast CI, devkit release train | Slower nix CI on the fleet runner; GPU work as jobs, not in CI; own cadence |

**The contract between them:**

- `scitadel-core` (published) supplies domain types: `Paper`,
  identifiers, `UntrustedText`.
- A protocol crate (`scitadel-hub-proto`) holds the request and
  response types (§7). It **lives in this repo** under
  `MIT OR Apache-2.0` and ships with the release train, so the
  client never depends on the hub repo and no third-party integrator
  inherits AGPL by `cargo add`-ing it. An AGPL proto crate would
  contaminate clients; this is the licence split, not a preference.
- Repo settings come from vig-os/org-config (otterdog), as for this
  repo.

The repo is created only after this ADR is Accepted (#313).

**Four repos in total:**

| Repo | Visibility | Licence | Contents |
|---|---|---|---|
| `vig-os/scitadel` | public | `MIT OR Apache-2.0` | Client (CLI, MCP, TUI, GUI, Tauri + web + mobile); `scitadel-core`, `scitadel-http`, `scitadel-adapters`, `scitadel-scoring`, `scitadel-hub-proto`, `scitadel-hub-fixtures`; spec docs |
| `vig-os/scitadel-hub` | public | `AGPL-3.0-only OR LicenseRef-scitadel-Commercial` | Hub server and hub-internal crates |
| `vig-os/scitadel-hub-deploy` | **PRIVATE** | internal | NixOS module (`services.scitadel-hub`), OCI image build, Helm chart |
| `vig-os/scitadel-cloud` | private, later | proprietary | Hosted-tier control plane: tenants, billing, metering, admin console |

**On-prem comes via the commercial grant.** Keeping the deploy repo
private is **deliberate friction** for other admins: a university or
lab installs the hub through the commercial licence or a support
contract, not by cloning a public module. The AGPL hub source stays
public. The product-angle review and the license-spike both
recommended a public `Apache-2.0` deploy repo; the owner overrode them
on the deployment-friction axis.

`gerchowl/g-fleet` consumes `services.scitadel-hub` from the private
deploy repo **pinned by tag**, never patching it locally. Hub secrets
live in their own `sops` file. `scitadel-cloud` is extracted at the
**first external tenant**, the **first billing code**, or when
**serving moves to Hetzner** — whichever comes first.

## 2 · Data model on tessera: layout and lifecycle

- **Shard products, not one product per paper.** A release is a
  collection of shard products of about 1–10 GB *(estimate)*:
  - OpenAlex: one shard per field (26) × publication-year bucket,
    grouped in a collection per field under the release's base
    collection;
  - patents: one shard per grant year × CPC section (A–H, Y).

  The blocks inside a shard are the document model of §9. Each product
  records its provenance (§11): the upstream release, the filter that
  defined the subset, and the code version that built it.
- **Base + deltas + a pointer** (H6, #320):

  ```
  base/<release>/     a collection per quarterly snapshot (OpenAlex) or
                      PatentsView release (patents): shards + derived products
  delta/<date>.tsra   the same blocks: upserted rows + tombstones
                      (id, merged → W… | deleted), with small caches of its own
  current             a tiny content-addressed collection {base, deltas[]};
                      the only thing that moves
  ```

  OpenAlex deltas are daily (§5); patent deltas are the weekly grant
  XML (§6).
- **Reads are newest-wins.** The logical table is base ⊕ deltas: the
  newest row per ID wins and tombstones drop or redirect IDs. In
  DataFusion this is a union plus an anti-join on the small delta ID
  set. Search runs on the base and delta caches, merges the top-k and
  masks superseded rows. tessera has no keyed delta semantics yet:
  tessera#781 (upsert/tombstone/redirect, newest-wins read,
  compaction, retention).
- **Compaction writes a new base.** A new quarterly release becomes the
  new base, the deltas older than its snapshot date retire, and
  `current` moves atomically. Deltas may also be merged monthly along
  the way. Nothing is rewritten in place.
- **Retention is an explicit policy.** Hub data lives in its own
  versitygw bucket on the dedicated NixOS ZFS + versitygw box
  (g-fleet#1363, replacing the shared path of g-fleet#1121), with a
  retention policy (default: keep the last 2 bases and their deltas).
  The fleet's add-only rule for `/srv/anvil` covers research data
  written by GPU jobs. The hub's working copy is a rebuildable mirror
  of the bucket, and a generation expires by that policy, never by an
  ad-hoc delete. How the two rules meet is settled in g-fleet#1363 (H6).
- **Derived means rebuildable.** Every product can be rebuilt from
  upstream or from the artefact store (§12). That is why tessera's
  pre-1.0 format is acceptable here: a format change costs a rebuild,
  not data.
- **Size blocker: tessera#628.** String-heavy tables seal about 3×
  larger than Parquet today. The architect triaged it as a format
  decision (a determinism test on ksb, then an ADR). Until it lands,
  sealed shards are estimated at about 3× their Parquet size.
- **Internal Parquet staging until tessera is ready.** Until tessera
  W0/W1 land (tessera#778), the hub may stage ingest as Parquet on its
  own disk and re-seal later. That staging is never published: what
  the hub publishes is `.tsra` only (§6).

## 3 · Query engine: DataFusion

DataFusion is tessera's public query API (tessera ADR-0062), is pure
Rust and embeddable, reads OpenAlex's Parquet natively at ingest, and
reaches the object store through the `object_store` crate. DuckDB would
add a C++ dependency and a second engine for no capability we need.

**Blocker:** today `tessera sql` materialises a whole table block in
RAM. The hub needs the streaming provider with filter pushdown and
multi-product scans: tessera#654, implemented by #666 and #668.

## 4 · Retrieval: BM25, vectors, graph

All three are tessera derived caches under tessera ADR-0054: unsealed,
rebuildable, stamped with the seal of the product they index, and
fresh when the stamp matches. ADR-0054 itself is still Proposed
(tessera#365).

- **BM25.** A tantivy index over a declared text column. The analyzer
  configuration is part of the stamp. It is queried as a DataFusion
  table function that composes with filters (tessera#775).
- **Vectors.** Embeddings are stored as native Vortex fixed-size-list
  columns (tessera#768), searched by exact scan, IVF-flat stored in
  Vortex, or an optional HNSW cache (tessera#769). Hybrid search fuses
  BM25 and vector ranks.
- **Graph.** Edges are typed Vortex tables:
  - work cites work;
  - patent cites patent;
  - patent cites paper;
  - work or patent has CPC/topic.

  An adjacency (CSR) cache serves algorithms such as co-citation and
  PageRank. Traversals are DataFusion recursive CTEs or a
  `neighbours()` function. Node embeddings (node2vec, GraphSAGE)
  become vector columns (tessera#776).
- **Then System One.** Retrieval returns a candidate set; #246's
  scorer (Clef/Kev) ranks it; only the shortlist reaches an LLM.

**Why no graph database.** Kùzu, the embedded graph database most
people reached for, is gone, and its Rust successors are young:
KyuGraph (a pure-Rust port in the Kùzu line, created February 2026),
NervusDB v2 (Cypher subset, binary releases "coming"), CypherLite,
Grafeo (GQL-focused; only self-reported benchmarks). A citation graph
is two ID columns, which fits tables plus an adjacency cache. Graph
access goes through a trait, so a Cypher engine can plug in later if
one matures.

## 5 · OpenAlex sync strategy

**Base.** The quarterly public snapshot, filtered to a subset by
topic, field or subfield, institution, source, or the citation
neighbourhood of the lab's libraries. It is read from the public bucket
with DataFusion over S3, without downloading the full dump. Size: 5–50
GB per subset *(estimate)*.

**Daily jobs, on one free key ($1/day ≈ 10,000 list calls):**

1. **Subscriptions.** For each subscribed topic, author, institution or
   source, a list query with
   `from_publication_date = last_run − lookback` (default 90 days),
   `per_page=100`, cursor paging. Every new work in all of OpenAlex
   would cost a few hundred calls, about $0.05/day *(estimate)*; a
   realistic lab set costs under $0.01.
2. **Rolling refresh.** Re-fetch known works 100 IDs per call
   (`openalex_id:W1|…|W100`), hottest first: works in members'
   libraries, recently read, recently cited. The rest of the budget
   refreshes about 950k works a day *(estimate)*, so a 5M-work subset
   turns over in about five days.
3. **On demand.** A single-entity GET (free) when a client opens a
   paper.
4. **Merges and deletions.** A refresh that returns a different ID, or
   nothing, marks a merge or a deletion. Each quarterly snapshot
   reconciles them from its `merged_ids` and `deleted_ids` files.

Every response's `X-RateLimit-Credits-Used` is logged. The key never
appears in logs or errors (#276).

**Accepted gaps:**
- A work indexed late, with a publication date older than the lookback
  window, waits for the next quarterly snapshot.
- Changes to works outside the subset are not tracked.

**Optional paid path.** With Member+ (or a Member plan's $20/day), the
hub swaps the subscription heuristics for `from_updated_date` sync.
The storage model is unchanged; only the source of deltas changes.

**Why not a free "live mirror" of everything.** The change feed is the
product OpenAlex sells. Reconstructing it from the free key would mean
re-fetching everything, against the spirit of their "free data, paid
services" model. A full mirror would also exceed anvil's disk (§8).

**Embeddings.** Existing CC0 sets come first:
`colonelwatch/abstracts-search` (MiniLM, 384-d) and
`GlobalCampus/openalex-multilingual-embeddings` (schemas not yet
verified). The hub computes embeddings only for works they miss.

## 6 · Patent corpus

- **Inputs:** `PVGPATDIS` TSVs and the weekly Grant Full Text XML,
  1976–present. Pre-grant publications and EPO DOCDB are later
  options.
- **Build.** Tables: patents, claims, description sections, CPC,
  citations patent→patent and patent→paper, assignees, inventors.
  Embeddings: PatentSBERTa (768-d) over title+abstract, about 8M
  grants, about 12 GB at fp16 *(estimate)*.
- **Weekly delta:** the new grant XML becomes a new delta product.
- **Same cascade and QC as papers** (§10, §13):
  - **T0** parses the USPTO full-text XML (ICE DTD) into the §9 model:
    abstract, description, claims, embedded tables, math and chemistry
    (MathML / CWU chemical structures), and the description of the
    drawings. The claim dependency tree becomes edges (claim → the
    claim it depends on).
  - **Drawings** are TIFF images. They take the figure path of §9:
    stored as JPEG XL lossless, embedded with Qwen3-VL-Embedding, and
    sent to the vision model at T3 when needed.
  - **PatentsView TSVs** supply the disambiguated bibliographic data
    (assignees, inventors, locations).
  - **Licence:** US patent text is public domain, so every tier,
    including T4, is allowed. Other offices (e.g. EPO) are a later
    extension, each with its own licence check.
  - **Out of scope:** pre-1976 patents, which are images only and would
    need OCR.
  - **The golden set has a patent stratum** (§13).
- **Serving.** The patent `.tsra` is **walled**: the hub ingests it,
  embeds it, indexes it and serves it over the `search` / `get` /
  `neighbours` protocol (§7). It is **not published on Hugging Face**
  — the corpus is the running service, not a public dataset, and
  shipping it in bulk would dilute the product the hub sells
  (freshness, licence-class gating, extraction-cascade curation,
  System One ranking). Attribution is kept on `search` and `get`
  responses per row (PatentsView tables CC-BY-4.0 with attribution;
  USPTO text public domain in the US). Size: 100–150 GB in total
  *(estimate)*, held in the hub's warm tier (§8).
- **The hub answers what HUPD and HF don't.** No public dataset
  combines US patent full text from 1976 to now with embeddings and a
  search index (HUPD stops at 2018 and has no embeddings). The hub
  closes that gap as a *service*, not as a dataset.
- **PatentRef stopgap.** scitadel's PatentsView adapter points at a
  configurable PatentSearch-compatible endpoint, defaulting to
  PatentRef (#268 thread). It is a third-party free beta with no
  service commitment. Once a hub with H1 exists, the hub answers
  patent searches and PatentRef is only the fallback.

## 7 · Client/hub protocol

| Call | Input | Output |
|---|---|---|
| `search` | query text **or** a client-supplied query vector (tagged with embedding model id, version and dims), filters, mode (`bm25` / `vector` / `hybrid`), limit | Snippet-only candidates (IDs, scores, snippets, provenance), each with the mode that produced it, plus a search-level `reranked: bool` |
| `get` | Up to 100 IDs | Records |
| `neighbours` | ID, edge kind, depth | Graph neighbours |
| `coverage` | — | Which sources, subsets and product versions the hub holds — a client reads this to decide when to fall through to a live API |
| `capabilities` | — (optionally an index id) | Per index: embedding model id, version, output dims, Matryoshka truncation applied, BM25 analyzer configuration, and the available rerankers — a client reads this to decide whether to send a query vector or fall back to text (#316) |

- **Two separate advertisements.** `coverage` says *what the hub
  holds*; `capabilities` says *how each index is built*. They change
  at different cadences — `coverage` on every delta or release,
  `capabilities` on an index rebuild — and clients cache them
  independently. Mixing them would force every embedder update to
  look like a coverage change.
- **Provenance on every response:** it names the product seal or
  version that produced it.
- **Untrusted text:** every text field arrives as `UntrustedText`
  (ADR-007 amendment, #287).
- **Fallback:** the client asks the hub only for what `coverage`
  covers, and falls through to the live API on a miss, an error, or a
  bounded timeout. Every result records `answered_by: hub | <source>`.
- **Model mismatch is explicit.** A client-supplied query vector
  whose declared model id, version or dims do not match the target
  index's `capabilities` response is rejected with an explicit
  protocol error — the hub never silently re-embeds it under the
  wrong model (§14).
- **Transport:** HTTP+JSON first. The types do not assume it.

## 8 · Infrastructure — staged

Serving and ingest move through three stages. Each stage's host
mix is set; the data model, protocol and licence gating are
unchanged across stages.

### Stage 0 — anvil (phase 1 demo)

- **Hot tier.** anvil's KC3000 NVMe holds the POSIX working copy under
  `/srv/anvil/projects/<project>/`. DataFusion, tantivy and GPU jobs
  read local files much faster than ranged S3 reads.
- **Warm tier.** A dedicated **NixOS ZFS + versitygw S3 box**
  (gerchowl/g-fleet#1363), replacing the shared research-data store
  path of g-fleet#1121, is the canonical S3-compatible store and the
  distribution point inside the fleet.
- **Cold tier.** A **Hetzner Storage Box** (encrypted, via `restic`)
  holds the backups and the derived products that fall out of the
  warm working set.
- **Ingest.** GPU work only through `gpu_submit`: declared `vram`,
  `priority=low`, chunked to the default runtime with checkpoints,
  outputs written to new directories only. No index stays resident in
  VRAM; queries are served from CPU and NVMe.
- **Tenant simulation.** Per-tenant hub instances for development and
  acceptance tests run on **microVMs** (`microvm.nix`) on anvil, one
  per simulated tenant, so isolation and metering code paths are
  exercised against the same packaging the hosted tier will use.

### Stage 1 — Hetzner (first hosted users)

- **Serving.** A **Hetzner AX** dedicated box runs the hub binary on
  CPU.
- **Warm store.** **Hetzner Object Storage** replaces the fleet's
  versitygw as the distribution point for `.tsra` products.
- **Cold.** The Storage Box of stage 0 stays in the cold tier.
- **GPU.** anvil remains the steady-state ingest GPU; **hourly-rented
  Hetzner GPU** covers backfills, re-embedding after a model upgrade
  and spikes.

### Stage 2 — hosted spaces

- NixOS **k3s + microVMs** host org and personal spaces with per-tenant
  isolation, metering and billing (behind `scitadel-cloud`, §17).
- Backfill and re-embedding continue on **hourly-rented Hetzner GPU**.

### On-prem

- The NixOS module, OCI image and Helm chart from the private
  `scitadel-hub-deploy` repo (§1). An on-prem hub runs the same
  binary and the same `.tsra` shards as any stage above, and
  interoperates with the same protocol.

### GPU policy (all stages)

- **Steady state:** anvil's 5090 via `gpu_submit`.
- **Backfill and re-embedding:** hourly-rented Hetzner GPU, not anvil
  (anvil is shared with inference and must not be monopolised by a
  one-off re-embedding sweep).

### Disk budget (stage 0, *estimate*)

anvil's canonical drive is one 2 TB 990 PRO plus the KC3000 tier:
- patents: 100–150 GB;
- OpenAlex subsets: 5–50 GB each;
- indexes: ~1.5× the text they cover;
- artefact store (§12): kept TEI for a 2M-paper subset ~8–10 GB
  zstd; paywalled PDFs at ~1.5 MB each, growing with what the lab
  acquires;
- initial total: about 1 TB including working copies.

A full OpenAlex mirror is ~770 GB as Parquet and an estimated ~2.5 TB
sealed in tessera until tessera#628 is fixed, so it **does not fit on
anvil** and is not a goal; the sized-up Hetzner stages absorb it.

**Scoring.** System One scoring follows #246 (hosted Clef or local
Kev). Where Kev runs is a deployment detail of #246, not of this ADR.

## 9 · Document model

TEI, JATS, publisher HTML and PDF are **ingest sources** (raw input,
§12), not storage. Storage is Arrow-shaped tables in tessera/Vortex,
designed for column access, compression and embedding (H5, #319).

**Blocks in a shard product,** each sorted by `(work_id, ord)`:

| Block | Columns |
|---|---|
| `works` | work_id, DOI, licence class, source (`jats` / `html` / `tei` / `pdf`), extractor@version, per-work content hash, raw-input hashes + URLs, QC metrics (§13) |
| `elements` | work_id, ord, parent_ord, kind (heading, paragraph, equation, list, caption, …), text, page, bbox[4] |
| `inline_refs` | work_id, ord, span, target kind (citation, figure, table, formula), target id |
| `bib` | work_id, bib_id, parsed fields, DOI, `matched_work_id`, the source of citation edges |
| `table_cells` | work_id, table_id, row, col, text, `num: f64?`, unit |
| `figures` | work_id, fig_id, caption ord, bbox, blob ref |
| `chunks` | work_id, chunk_id, element span, tokens: 300–400-token spans for #246 (derived) |

**Derived products next to each shard:**
- embeddings, one product per model: chunk and figure vectors,
  row-aligned with `chunks` and `figures` (tessera ADR-0062 row
  alignment; vector columns per tessera#768);
- figure blobs, in their own product so text scans never touch images
  (storage rules below);
- figure descriptions written by the T3 vision model (chart type,
  axes, variables, trend) as a derived text column for BM25 and text
  embeddings. These cover the hot set only, generated lazily, not the
  whole corpus;
- a work_id → shard index, so a citation to a work in another shard
  resolves cheaply (tessera#780).

**Figure storage is lossless only:**

| Source image | Stored as | Why |
|---|---|---|
| Vector graphics in the PDF | SVG, in a string column (zstd) | Exact and small |
| Embedded JPEG | JPEG XL lossless transcode | ~20% smaller, and the original JPEG is reconstructed bit-exactly, so its hash still verifies |
| Lossless raster (PNG, TIFF, patent drawings) | JPEG XL lossless | Smaller than PNG; no generational loss |
| 16-bit / high bit-depth | JPEG XL | WebP is 8-bit only and capped at 16,383 px per side |

- **Never re-encode lossily for storage.** WebP/AVIF thumbnails exist
  only as on-demand display views.
- **Rust:** jxl-oxide decodes; encoding goes through libjxl (C++). A
  pure-Rust lossless encoder is unverified.
- **tessera has no binary columns today,** by design. Declared
  media-typed binary columns are tessera#779.

**Figure embeddings: Qwen3-VL-Embedding** (`Qwen/Qwen3-VL-Embedding-2B`,
2048-d, or the 8B at 4096-d; released 2026-01, arXiv 2601.04720).
- It puts text, images and page screenshots in one shared space, so a
  text query finds a chart.
- Matryoshka truncation to e.g. 512–1024 dims keeps the store small;
  the quality loss is checked on the golden set (§13).
- The Qwen3-VL-Reranker reorders the top-k.
- CLIP and SigLIP were considered and are weak on charts and plots.
- **Where it runs:** bulk embedding on the 5090 via `gpu_submit`,
  interactive work on sage.

**Linking:**
- **Within a shard, by keys.** Sorting by `(work_id, ord)` keeps one
  paper's rows together in every block, so "give me paper W" touches a
  few chunks per block, found by chunk statistics (tessera ADR-0059).
- **Across products, by provenance** (§11).
- Declared keyed links between blocks (`bib.matched_work_id` →
  `works.work_id`) and the work_id → product index are tessera#780;
  binary columns for figure bytes are tessera#779. Both are tessera gap
  issues under tessera#778.

**Compression comes from the layout:**
- sorted keys compress to runs and deltas;
- `kind` and section names use dictionaries;
- prose uses string encodings (tessera#628);
- numbers stay apart from text: `table_cells.num`, bbox and pages are
  typed columns for pcodec;
- embeddings are f16 or int8 + scale (tessera#768).

Rough per-paper size *(estimate)*:

| Part | Size |
|---|---|
| PDF | ~1.5 MB |
| Text | ~5–8 KB compressed |
| Tables | a few KB |
| Figures | ~100 KB–1 MB, lossless (JPEG XL / SVG) |
| Vectors | ~10 KB |

Figures dominate what is left. H5 measures the real numbers.

**Rendered views are not storage.** Typst, HTML and Markdown are
generated on demand for the TUI reader, the GUI and LLM context.
Converting a PDF into Typst is lossy (layout, reading order,
equations) and the result is not queryable; storing it would also make
the Typst compiler a dependency for reading our own data.

**No per-paper `.tsra` in the hub.** Per-paper products would have
three costs:
- their overhead (manifest, seals, footers) would exceed a paper's
  ~20 KB of text;
- tiny blocks compress badly;
- a corpus-wide query would open millions of objects.

A per-paper `.tsra` exists only as the output of a client-side
`detach` (§12).

## 10 · Extraction pipeline

**Source preference per work:**
1. JATS (PMC / Europe PMC bulk);
2. publisher HTML;
3. Grobid TEI from the OpenAlex content API. OpenAlex holds ~71M TEI
   files (~1.5 TB, ~73M PDFs at ~113 TB), but each download costs
   $0.01, so a free key fetches 100 a day. It fills gaps only;
4. our own PDF extraction.

**Tables** come from JATS first. **Figures** are cut by their
coordinates and stored losslessly (SVG, or JPEG XL; §9), with captions
linked. **Formulas** come as
LaTeX/MathML from JATS or from math OCR; Grobid's flattened formula
text is a fallback only. **Chunks** of 300–400 tokens feed #246 pass 2.

**A tiered cascade.** Each tier runs only on what the cheaper tier
could not settle:

| Tier | What | Where | Escalates when |
|---|---|---|---|
| T0 | Parse JATS / publisher HTML / USPTO XML | CPU | No structured source, or a QC flag |
| T1 | Tool extraction: Grobid, Docling, Marker, math OCR | CPU + the 5090 (`gpu_submit`) | A QC flag, or two extractors disagree |
| T2 | System One yes/no checks (Clef/Kev, #246): "is this table complete?", "is this the reading order?" | Clef hosted / Kev local | The answer is below the calibrated confidence |
| T3 | A Qwen vision-language model compares the page image with the extraction and repairs it (model size to be decided) | sage | It cannot repair, or its confidence is low |
| T4 | Opus (`claude-opus-5-5`) via the API gives the final verdict | external API | Fails → quarantine + a red fixture (§13) |

**Guardrails:**
- **Tiers are licence-gated.** T4 is an external API: only CC-BY/CC0
  and public-domain content (US patents) may reach it. Paywalled or
  unlicensed content stops at T3, which runs on our own hardware,
  unless its licence allows more.
- **T3 and T4 have budget caps,** and work queues when a cap is reached;
  nothing is skipped silently.
- **Every tier's verdict goes into provenance** (§11): model and
  version, prompt hash, decision, confidence.
- **Escalation thresholds are calibrated** against the golden set
  (§13), and recalibrated when a tier's model changes.

Patents run the same cascade with a USPTO-specific T0 (§6).

## 11 · Provenance of every process

- **Every derived product records** (tessera Generation /
  `derived_from`):
  - tool and version, and config;
  - model-weights hash;
  - input hashes (raw artefacts or upstream products);
  - GPU job ID and host;
  - the cascade verdicts that shaped it.
- **Per work, as columns on `works`:** which source and extractor
  produced the rows, and the highest cascade tier reached.
- **Raw inputs are referenced by hash and URL even when not held**
  (§12). tessera ADR-0033 assumes originals are kept, so tessera
  provenance needs records for external inputs it does not hold
  (held / cold / discarded, with a `--fetch-check`): tessera#782,
  which amends ADR-0033. Today a blake3-digest source reference
  *fails* `tessera provenance --require-complete`.
- **Multi-step and per-row provenance.** One shard passes through
  several tools and cascade tiers, and different works reach different
  tiers. tessera#782 also adds typed multi-step pipeline provenance and
  per-row provenance via a `prov_id` column. Today tessera records
  provenance only per product.
- **Originals are never touched.** Re-extraction or re-embedding writes
  new derived products.

## 12 · Artefact store, raw retention, licences and the client store

**Heavy originals stay out of the corpus database** (H4, #318). PDFs
and raw JATS/TEI/HTML live in a content-addressed artefact store:
- keys are blake3 hashes, the identity tessera and ADR-007 already use;
- it is a cold bucket on the dedicated versitygw box (g-fleet#1363);
- `works` rows reference it by hash and source URL;
- raw XML never goes into tessera products.

**Raw retention is set by size and by the cost of fetching again:**

| Raw input | Policy | Why |
|---|---|---|
| Open-access PDF | Delete after ingest. Keep blake3 + URL + fetch date; a re-fetch is verified against the hash. | Large (~1.5 MB), free to fetch again |
| Paywalled, acquired PDF | Keep, in cold storage, as the version of record | Hard to obtain again |
| JATS from PMC / Europe PMC bulk | Either way; default keep (small) | Free to fetch again |
| TEI from the OpenAlex content API | Keep, zstd-compressed: ~40 GB raw, ~8–10 GB zstd for a 2M-paper subset *(estimate)* | $0.01 per fetch |

Keeping the XML lets us re-derive after extractor upgrades without
paying again, and protects against upstream link rot and retractions.

**Licence classes, recorded per artefact (per-row column on every
product):**

| Class | Hub cache | Embed / index | Cascade up to | Served to |
|---|---|---|---|---|
| CC-BY / CC0 / public domain | yes | yes | T4 | every tier (free local, lab, org, hosted paid, on-prem); attribution kept on `search` and `get` |
| CC-BY-NC | not in any paid hosted tier | — | — | **excluded** from every paid hosted tier (CC-BY-NC 4.0 §2(b)(1); a 2048-d embedding counts as adapted material) |
| CC-BY-ND | not in redistributed indexes | — | — | **excluded** from paid hosted tiers until EU case law resolves whether a lossy embedding is "adapted material"; treat as derivative to be safe |
| Free to read, no licence (bronze/green) | lab-internal only | internal only | T3 | lab / research-org on-prem only |
| Paywalled, obtained with a member's access rights | entitled users only (same licence, e.g. ETH members) | internal only | T3 | entitled users only |

**Legal basis for the mining, by tier (not legal advice).**

- **Research organisations (ETH, lab on-prem).** EU DSM Directive
  (EU) 2019/790 **Art. 3** (scientific TDM, no opt-out, reserved for
  research organisations and cultural-heritage institutions) plus
  Swiss CopA **Art. 24d** (scientific TDM on lawfully-accessed works)
  cover ingest and internal indexing. Art. 24d does **not** permit
  **commercial redistribution** of mining results beyond verification.
- **Commercial hosted tier (`scitadel-cloud`).** Relies on EU DSM
  **Art. 4** only (Art. 3 is reserved for research organisations), so
  it must honour every machine-readable **opt-out** (robots.txt,
  TDMRep, `noai`, ai.txt), **re-checked on every re-ingest**. This is
  a separate issue under #311.
- **Database sui generis right.** OpenAlex's CC0 declaration waives it
  for its subsets, so republishing a subset is permitted; every
  `.tsra` pins the upstream snapshot date for audit.
- **GDPR.** A **DPIA** is written and reviewed, and an **Art. 14
  notice** published, before the first hosted user. ORCID / author
  data re-use under Art. 6(1)(f) legitimate interests; erasure
  requests honoured (soft-delete from index, retain hash for dedup).

The free-to-read row relies on the lab's reading of CopA Art. 24d,
to be confirmed with ETH Library (open question 7).

**The client store model:**
- **The client keeps references.** A local scitadel stores identity
  (DOI, OpenAlex ID, content hash), annotations, reviews and its
  configured hub. By default it stores no PDF bytes.
- **The hub keeps bytes, once per lab.** When a client acquires a work
  through ADR-007 (including tier-2 TDM #258 and tier-4 capture #256),
  it offers the artefact to the hub (upload-on-acquire). The upload
  carries the licence class, the entitlement and the acquisition
  provenance. Entitled colleagues then get it from the hub instead of
  the publisher.
- **`scitadel detach` / `export`** pulls full copies of tracked papers
  into local storage, checking the licence per artefact. It can emit a
  per-paper `.tsra` whose provenance points at the hub shard's seal.
- **Guardrails:**
  - never a bulk proxy for paywalled content: the hub caches only what
    members fetched individually, and the ADR-007 pacer still applies
    upstream;
  - minimal access logging: entitlement checks, not reading histories,
    with bounded retention.

## 13 · Quality: golden set, red/green loop, random-sample QC

(H7, #321.)

- **Golden set.** A stratified fixture set of about 500–2,000 papers:
  - strata: source type (JATS / HTML / TEI / PDF), publisher, field,
    year, language;
  - hard cases: math-heavy, table-heavy, scanned, two-column,
    supplementary material, retracted;
  - **a patent stratum:** utility, design, plant and reissue patents;
    chemistry-heavy, many-claims and drawings-heavy patents.

  Each fixture is the raw input plus the expected tessera output, as
  `insta` snapshot tests.
- **Split by licence.** CC-BY/CC0 and public-domain fixtures live in
  the `scitadel-hub-fixtures` crate (§16, open question 2) and run in
  public CI. All others live in a private fixture bucket and run on
  fleet runners only.
- **The red/green loop.** Every extraction failure or QC flag goes
  through the same steps:
  1. quarantine the raw input;
  2. add a fixture with a failing test (red);
  3. fix it (green);
  4. the case joins the golden set permanently.

  The hub may open the issue itself, respecting the licence split.
- **QC metrics per work,** as columns on `works`:
  extracted-text-to-PDF ratio, empty sections, table parse rate,
  extractor confidence.
- **Random-sample QC on every batch,** for silent errors:
  - **Ingest:**
    - a fixed random sample per batch goes through T3/T4 (licence
      permitting) regardless of flags. 300 samples with 0 errors put
      the error rate below ~1% at 95% confidence (rule of three);
    - error rates are tracked per source and per extractor version.
  - **Embeddings:**
    - NaN, norm and duplicate checks;
    - self-retrieval;
    - recall@k on golden queries;
    - ANN recall against exact search (tessera#769);
    - drift between model versions on the golden set before any
      re-embedding.

  Confirmed errors become red fixtures.
- **Upgrades go through the golden set first.** An extractor, model or
  threshold change runs on the golden set and produces a diff report.
  Only after review is the corpus re-derived.
- **Patents too.** The same harness covers the patent corpus (§6).

## 14 · Serving model: where compute runs

The hub separates **ingest** (GPU, queued, latency does not matter)
from **serving** (CPU, interactive). No index, embedder or reranker
is resident on the 5090; every GPU use goes through `gpu_submit`
(§8). This matches anvil's shared-GPU rule: the 5090 is held by
inference workloads, so the hub cannot rely on it for a search's
tail latency.

**Per-step placement** *(latency estimates)*:

| Step | Where | Latency *(estimate)* |
|---|---|---|
| Filters / metadata (DataFusion over tessera) | Hub, CPU + NVMe | ms to tens of ms |
| BM25 (tantivy) | Hub, CPU | <50 ms at 10⁸ docs |
| Query embedding | Client or hub, CPU | ~10–50 ms with a small text model (e.g. `Qwen3-Embedding-0.6B`); ~0.2–1 s for the 2B VL embedder on a laptop CPU |
| ANN search | Hub, CPU + RAM or NVMe | ms |
| RRF fusion, filters, graph hops | Hub, CPU | ms |
| Rerank of the top 50 (`Qwen3-VL-Reranker`) | Optional; hub GPU queue or sage | ~0.1–0.5 s on GPU |
| System One scoring of the shortlist (#246) | Client-side Kev, or hosted Jev | Kev-4B ~300 ms for 5 questions on a 32 GB Mac |
| LLM reading pass | API, or local | varies |

**Rerank fallback.** If the GPU queue is busy, the hub returns
unreranked results flagged `reranked: false` rather than block the
search. The client may choose to wait and re-request, or score the
shortlist with #246 directly.

**The wire carries references, not documents.** The client receives
IDs, scores and snippets. Bytes flow only on `open` or `detach`
(§12), so a search round-trip stays small even for a long
shortlist.

**Client-side query embedding is the default.** It offloads the
hub, works offline against detached subsets (§9, §12), and keeps
the exact Matryoshka truncation the index was built with. The hub's
**`capabilities` response** (§7, #316) advertises, per index: the
embedding model id, version, output dims, Matryoshka truncation
applied, the BM25 analyzer configuration, and the available
rerankers. (`coverage` is a separate call that lists what the hub
holds, not how each index is built — they change at different
cadences and clients cache them independently.) The hub **rejects a
query vector whose declared model id, version or dims do not match
the target index** with an explicit error, rather than embedding it
under the wrong model and silently degrading recall. Hub-side query
embedding (text in, hub embeds) remains a fallback for thin clients
and for the window between an index re-embedding and a client
refresh.

**Hub sizing** *(estimates)*. A 50M-chunk corpus at 512-d fp16 is
about 51 GB of raw vectors — resident in RAM on a sized box, or
served through an IVF-PQ / DiskANN-style on-disk index at a few GB
RAM plus NVMe. A lab of ~20 users is trivial for a CPU hub:
tantivy and the ANN cache both sustain hundreds to thousands of
QPS per core, and search is compute-bound on fusion and scoring,
not on I/O.

**Offline mode.** A detached topic subset with its own indexes
(e.g. 50k papers: text, chunk embeddings and BM25, ~1–3 GB
*(estimate)*) gives fully local millisecond search on a laptop.
Remote reads of a hub's `.tsra` over HTTP range requests (tessera
ADR-0034) suit column scans and `get`-by-ID, not BM25 or ANN random
access — those want the subset on local NVMe, which `detach`
arranges (§12). The protocol routes a search to a local subset when
one is available for the query's topic, and to the hub otherwise;
`answered_by` reports which path answered (§7).

## 15 · Sequencing: build first, licence-clean

The owner's decision (2026-11-08): BUILD FIRST on a strictly
licence-clean corpus, then approach ETH Library with a working
demo. No licensed/paywalled caches, no grey-area sources, no
scraping, in phase 1. Within that envelope phase 1 goes for
**maximum depth** (full text, figures, tables, cross-links,
embeddings for as much as we can fit), not minimum viable. The
corpus is **walled**: served by the hub, not published in bulk on
Hugging Face. What is published is the toolchain — the client, the
protocol, the conformance kit, and the `scitadel-hub-fixtures` CC0
slice (open question 2 in §16).

### Phase 1 — licence-clean sources

Only sources whose content is **CC0, CC-BY or public-domain per
item**, with per-item filterability, and whose access terms allow
bulk harvest, are in scope. The triage in #323 is load-bearing;
one line per source here, full matrix there.

**Already locked in by shipping issues:**

- **USPTO Grant Full Text XML** (#314) — PD-USGov, via ODP Bulk Data
  Directory on `data.uspto.gov` (per #323's top comment), weekly;
  100 % PD; full text + drawings + MathML.
- **USPTO Pre-Grant Publication XML** — PD-USGov; weekly; same ICE
  DTD; captures filings that never issue. Folded into #314 as a
  second product sharing the H5 cascade.
- **PatentsView PVGPATDIS** (#314) — CC BY 4.0; quarterly; US
  disambiguated inventors / assignees / locations / CPC / citations.
- **OpenAlex quarterly snapshot subset + free-key sync** (#315) —
  CC0; abstracts (inverted index), no FT.

**New phase-1 sibling slices (file as sub-issues under #311):**

- **PMC OA `oa_comm` packages** — JATS tables and MathML + the
  original figure files, filtered per article to CC0 / CC BY only;
  biomedical FT backbone (H9).
- **SureChEMBL** — CC BY 4.0; every 2 weeks; SMILES/InChI + patent →
  compound maps; drops straight into the patent corpus (H10).
- **DataCite + Crossref metadata** — CC0 (DataCite) + public-domain
  metadata graph + references (Crossref); publish the graph, strip
  publisher-supplied abstracts (H11).
- **OpenAIRE Graph** — CC BY 4.0; Zenodo v10.5.0 (321 GB); EU
  funding linkage (FP7 / H2020 / Horizon Europe) + Scholix (CC0).
- **OpenCitations Index + Meta** — CC0; 2.56 B citations (dump
  2026-07-28); strictly more open-citation coverage than Crossref's
  own refs subset.
- **DOAJ** — CC0 article + journal metadata; monthly public dump.
- **DBLP** — CC0; monthly DOI'd XML snapshots on DROPS; sharper
  author disambiguation for CS.
- **ACL Anthology, 2016+ CC BY subset** — CC BY 4.0; metadata XML
  - `anthology.bib.gz` + per-paper PDFs. Pre-2016 NC-SA is
  cache-internal only.
- **DOAB / OAPEN** — CC0 metadata + per-title CC0/CC-BY book FT
  subset; exclude NC/ND; CC-BY-SA cache-internal.
- **ORCID Public Data File** — CC0; annual on Figshare; identity +
  affiliations + works list.
- **OSF Preprints, CC0/CC-BY subset** — PsyArXiv / SocArXiv /
  EarthArXiv / engrXiv / LawArXiv / MetaArXiv full text filtered
  per preprint.
- **HAL metadata** (CC0) + **HAL file-level CC-BY/CC0 subset** only
  after the Jan-2026 mandatory-licence rollout propagates; legacy
  no-licence files excluded.
- **Research Square** — editorial policy CC BY 4.0 across the
  corpus; include subject to confirming the request-API permits
  bulk harvest.
- **Zenodo, CC0/CC-BY per-record subset** — metadata CC0; monthly
  dumps via `/api/exporter`; lowest-friction add after OpenAlex
  and PMC OA.
- **ChemRxiv, CC-BY subset only** — include subject to re-verifying
  the public-API endpoint and TDM rate limits.
- **arXiv CC-BY/CC0 subset** — LaTeX **source** via bulk S3 for
  papers whose per-paper licence is CC-BY/CC0; perfect equations,
  tables, often-vector figures. Non-redistributable majority stays
  out of phase 1.
- **bioRxiv + medRxiv CC-BY/CC0 subset** — MECA packages (JATS +
  figures), filtered per paper.
- **Europe PMC CC-BY/CC0 subset** — same per-article licence model
  as PMC OA.
- **INSPIRE-HEP metadata** — CC0; HEP citation graph (not FT —
  duplicates arXiv).
- **Retraction Watch via Crossref** — QC must-have (see below).
  Licence footnote: treat as **CC BY 4.0 "data via Crossref /
  Retraction Watch"** until Crossref states otherwise.

### Phase-1 ingest shape — maximum depth

Within the licence-clean envelope phase 1 goes deep, not shallow:

- **Structured sources first (T0 — cheap, best quality):** JATS /
  LaTeX / XML sources are ingested as native structure (equations,
  tables with MathML, figure references, bibliographies), not
  reduced to plain text. The GPU-heavy extraction tiers (Docling /
  Marker / math OCR under §10 T1–T3) are reserved for PDF-only
  items. Sources in this bucket: PMC OA `oa_comm` (JATS + original
  figures); arXiv LaTeX source (CC-BY/CC0 subset); bioRxiv /
  medRxiv JATS; US patent XML; ACL / OAPEN / DOAB / Zenodo CC-BY
  items.
- **Crossref TDM-link harvester** for CC-BY/CC0 publisher full text:
  filter Crossref metadata to CC-BY/CC0 `license` URLs and follow
  the `link[intended-application=text-mining]` entries through
  scitadel's paced per-publisher buckets (ADR-007). This is the
  main non-biomed lever for ETH fields: MDPI, Frontiers, Copernicus,
  IOP, APS (PRX etc.), Springer Nature OA, IEEE Access, ACS
  AuthorChoice CC-BY. Open question 12 checks that Crossref still
  exposes both fields and notes per-publisher terms.
- **Staged storage, not HF.** The corpus is walled (decision 19):
  it is held and served by the hub, not shipped to Hugging Face.
  Stage 0 (§8) keeps the full set across tiers inside the fleet: a
  **Hetzner Storage Box** is the cold tier via `restic`, the
  versitygw box (g-fleet#1363) is warm, and anvil's KC3000 is the hot
  working set. Stage 1 moves the warm tier to **Hetzner Object
  Storage** and serving to a Hetzner AX box. Sizing *(estimates)*:
  ~5–10 M licence-clean papers × (text ~8 KB + tables ~5 KB +
  figures ~200 KB JXL + vectors ~30 KB) ≈ 1–2.5 TB; US patent
  drawings ≈ 0.5–1.5 TB. This exceeds anvil's 2 TB 990 PRO, which is
  why the Storage Box / Object Storage tiers exist, not a public HF
  cold tier.
- **Embedding levels** — work, section, chunk, figure (Qwen3-VL),
  table (serialised + caption → text embedder), independent claim,
  graph node. GPU budget *(estimates)*: ~250 M text chunks ≈ 1–2
  days on the 5090; ~100 M+ figures / drawings with the 2 B VL
  embedder ≈ days–weeks — staged (hot topics first, the rest at
  `priority=low` in the shared queue).
- **Retraction flags are a must-have, not an enrichment.** Every
  record the hub holds that matches a Retraction Watch entry (by
  DOI or PMID) is surfaced with its retraction status and date on
  `search` and `get` responses; search result rendering says so.
  Flags are joined at index-build time (§5, §6), not at query
  time, and are first-class columns on the `.tsra` shards the hub
  serves.

### Phase 1 — gated features designed but disabled

The licence-gating code paths are wired through the schema and the
protocol so phase 2 is a configuration change, not a rewrite:

- **H4 (#318) entitlement classes** — licence-class columns on every
  row and every artefact; the protocol carries them. Phase 1 writes
  them with a single class (`public-licence-clean`), and `get` /
  `search` cannot return rows outside that class. The infrastructure
  for additional classes is designed; no code path that reads caches
  or serves non-CC0/CC-BY content is enabled.
- **Local display of licensed content** for entitled lab users —
  schema, retention policy (§12) and `detach` shape are designed;
  no licensed content is ingested in phase 1, so no reader path
  exercises them.
- **COUNTER 5.1 / SUSHI reporting** for publisher usage — the event
  schema, hub-side counters and the SUSHI emit point are designed;
  the recording path is a no-op when no licensed content is served
  (all phase-1 content is CC0/CC-BY/PD and not COUNTER-reportable).

### Phase 1 — deferred until the ETH Library conversation

Defer everything that needs an agreement, a legal reading, or a
publisher contract. We approach ETH Library with a running hub,
not a slide deck.

- **ETH Research Collection bulk harvest** — the Library
  rate-limits off-network bulk callers; harvest needs an agreement
  (coordinator-verified facts in #323).
- **CORE full text** — CORE's own licence is under review and the
  per-item licence of underlying works is not tracked at CORE's
  level.
- **Semantic Scholar / S2ORC** — the AI2 Datasets API licence blocks
  redistribution; even non-commercial internal use needs ETH legal
  clearance.
- **Unlicensed bronze / green OA full text** — free-to-read but no
  explicit CC licence; falls under CopA Art. 24d, needs the lab's
  legal reading before caching.
- **Paywalled caches** — anything behind a publisher paywall, via
  any vehicle (library IP, SUSHI, OpenAthens, …); phase-2 by
  definition.
- **EPO paid bulk** (DOCDB / INPADOC / EP full text) — subscriber
  contract forbids redistribution; cache-internal only if the lab
  subscribes.
- **Swiss institutional repositories** (EPFL Infoscience, ZORA,
  BORIS Portal, SERVAL) — all roll into the ETH-Library
  conversation as a Swiss repository federation discussion. EPFL's
  CC0 metadata is phase-1-eligible in principle but subsumed by
  OpenAlex's EPFL coverage, so no net gain.
- **NASA ADS, RePEc, Software Heritage (source code), Preprints.org,
  BASE** — each fails on licence, access channel or terms; see
  the matrix in #323.

### Phase 2 — what the ETH Library conversation unlocks

In a single conversation, not before, and only after a running
phase-1 demo:

- a **Research Collection harvest agreement** (ETH content, now
  licence-cleared and rate-limited properly);
- the lab's reading of **CopA Art. 24d** for internal text/data
  mining of free-to-read works (unlocks unlicensed bronze / green
  for internal use);
- a **publisher local-hosting addendum** negotiated through the
  **Consortium of Swiss Academic Libraries** (the vehicle for
  publisher paid caches, EPO bulk and Swiss repository
  federation);
- **COUNTER 5.1 / SUSHI** reporting plumbing — the recording path
  designed in H4 switches from no-op to live.

Nothing in phase 2 reopens the phase-1 licence surface: the walled
phase-1 corpus stays CC0/CC-BY/PD by definition, and phase-2 licensed
content goes into a separate licence class that is lab-internal only.

## 16 · Licensing

**Not legal advice.** Every reading below is to be verified with
counsel before the first external hosted user. The full consolidated
reviewer notes live in `license/licensing.md` (three parallel fresh
reviewers under the `license-spike` skill, 2026-11-08); this section
is a decision record, not the review.

### Per-part decisions

| Part | Licence |
|---|---|
| scitadel client (CLI, TUI, MCP, Tauri, libs) | `MIT OR Apache-2.0` (unchanged) |
| scitadel-hub server | **Dual: `AGPL-3.0-only OR LicenseRef-scitadel-Commercial`**, with written AGPL §7 additional permissions for client / cloud interop |
| `scitadel-hub-proto` crate | `MIT OR Apache-2.0`, in the scitadel repo (resolves the old open question 1) |
| Protocol prose spec | Community Specification License 1.0 (CC-BY carries no patent or trademark grant and is wrong for a wire-protocol spec a competitor may implement) |
| Conformance test vectors + verifier | CC0-1.0, in a sibling repo so re-licensing the hub cannot drag the vectors with it |
| NixOS module, OCI image, Helm chart | Proprietary / internal, in the **PRIVATE** `scitadel-hub-deploy` repo (§1) |
| `scitadel-cloud` control plane | Proprietary, extracted at the trigger in §1 |

### Rationale on the hub dual

AGPL alone does not stop a cloud provider from re-hosting the hub as
a service (the Elasticsearch fork proved this), and AGPL blanket-bans
in enterprise procurement would close the first-customer door. The
dual preserves the AGPL deterrent for self-hosters while giving
procurement-constrained customers a commercial grant without
re-opening the licence debate per deal.

### Contribution model

Solo author for now (≤24 months is legitimate for a one-person
project); `CONTRIBUTING.md` says "no external contributions
accepted; please open an issue instead". **At the first external
PR, switch to the Apache ICLA** — the hub dual-licence relicensing
path needs an ICLA's copyright and patent grant, so DCO alone is
insufficient.

### Trademark

Register `scitadel` as a word mark at **IGE Switzerland** in **Nice
classes 9 and 42** (approx. CHF 550), after a TMview + IGE + USPTO +
WIPO clearance search (`Citadel` is heavily used in finance and
gaming; class specificity matters). Extend via Madrid Protocol to
EUIPO when revenue arrives.

### Release-train obligations

- **`cargo-about`** NOTICE file wired into the release train.
- **`cargo-cyclonedx --format json`** SBOM published with every
  release.
- **`cargo-deny`** on the client tree with an allowlist and an
  **explicit copyleft deny** (`GPL-*`, `AGPL-*`, `LGPL-*`, `SSPL-*`,
  `BUSL-*`) — the client must not accidentally pull in copyleft and
  break App Store or Homebrew distribution.

### Model-weight policy

- **Qwen3-VL 2B / 8B**: Apache-2.0 on the HF model cards as of
  2026-11-08. Commercial OK.
- **Grobid**: Apache-2.0 code; DeLFT model files pinned per release
  pending audit.
- **Docling**: MIT. Commercial OK.
- **Marker**: Apache-2.0 code since v2.0.0 (2026-07); weights carry a
  revenue / funding threshold. Fine today; weights are **fetched at
  first run via a signed manifest**, **never baked into the OCI
  image**. Budget a commercial Marker key before scaling.

### Open-question resolutions referenced here

- Open question 1 (protocol crate home) resolved to this repo under
  `MIT OR Apache-2.0` by decision 18.
- Open question 2 (old "HF org") replaced by the `scitadel-hub-fixtures`
  CC0 slice question, superseded by decision 19.

### Not covered by this section

Operational legal items (trademark clearance search, BIS export
notification for encryption source, DPIA, Art. 14 notice drafting,
publisher TDM-term audits) are owner actions, written up next to
this ADR in `license/`, not in the ADR.

## 17 · Client app and tiers

The client is one product in three builds:

- **Tauri 2 desktop** (macOS, Linux, Windows).
- **Tauri 2 mobile** (iOS, Android).
- **Web dual build**, from the same Rust backend, via typed
  transport (`specta` / `tauri-specta`). The web shell is the
  entry point for hosted tiers.

All three builds share one Rust backend. The CLI, the TUI and the
MCP server are the same engine with different front-ends.

### Tiers

| Tier | Where it runs | Scope |
|---|---|---|
| **Free, local** | A personal machine | Offline-first; local SQLite; the hub source adapter points at live APIs or at any hub the user is entitled to. No prices in this ADR. |
| **Personal / Lab / Org — hosted** | Web (and the web dual build of the client) | Per-tenant space on `scitadel-cloud` (§1). Billing and metering out of scope here; see H9 and the business docs. |
| **On-prem** | Customer hardware | The NixOS module / OCI / Helm from the private `scitadel-hub-deploy` repo (§1), under the commercial grant or a support contract. |

### `detach` vs bulk export

- `scitadel detach` pulls **full copies of a user's own tracked works**
  into local storage (§12), checking the licence per artefact. This is
  the offline-first affordance.
- **Bulk export of the processed corpus is not offered** — the walled
  corpus (decision 19) is the hub's running service, not a dataset
  download. The two statements are consistent: individuals take their
  own workspace with them; the corpus stays behind the wall.

No tier prices and no tier-specific price ranges are recorded in this
ADR; see the owner's business docs for that.

## 18 · Research analytics (later)

A later expansion layer: **DataFusion over works, authorships
(ORCID), institutions (ROR), funders, and citations**, as a
WoS-Research-Intelligence-equivalent surface for research offices
(funding matching, collaborator graphs, benchmarking). No design
is proposed here; only the shape, so the ADR is complete across
the owner's decisions.

### GDPR guardrails (not legal advice)

Benchmarking **named individuals** is profiling under GDPR
Art. 4(4), so this layer defaults to **aggregates**. Individual
views are available **only to the researcher themselves or with
consent**. If outputs could feed hiring or funding decisions,
Art. 22 applies. Any public ranking surface follows
**DORA / CoARA** responsible-metrics principles.

### Funding-call matching (shape only)

Embeddings over call-text corpora from NIH RePORTER, NSF, CORDIS
and SNSF (each with its own licence check before caching). The
owner's business docs carry the when; this section carries the
what.

## Slices

| Slice | Issue | Where | Depends on |
|---|---|---|---|
| H0 — repo split + protocol crate home | #313 | both | This ADR Accepted |
| H1 — patent corpus (walled, served from the hub) | #314 | hub | H0; tessera W0/W1 (internal Parquet staging first is fine), W2 for indexes; g-fleet#1363 |
| H2 — OpenAlex lab mirror | #315 | hub | H0; tessera W0 (#628) and W1; g-fleet#1363 |
| H3 — `hub` source adapter + protocol | #316 | scitadel | H0; a mock hub for tests, H1/H2 for end-to-end |
| H4 — artefact store, raw retention, licence gating, upload-on-acquire, `detach` | #318 | both | H0, H5; ADR-007 acquisition; g-fleet#1363 |
| H5 — document model + tiered extraction pipeline | #319 | hub | H0, H4, H7; tessera W0–W2, tessera#779, #780, #782 |
| H6 — base + delta lifecycle, compaction, retention | #320 | hub | H0, H5; tessera#781; g-fleet#1363 |
| H7 — golden set, red/green loop, random-sample QC | #321 | hub | H0; runs alongside H1, H2 and H5 from their first batch |
| H8 — client app (Tauri 2 desktop + mobile; web dual build) | sub-issue under #311 | scitadel | H0, H3 |
| H9 — accounts, tiers and metering | sub-issue under #311 | scitadel-cloud | H8; extraction trigger of §1 met |
| H10 — tenant simulation on microVMs | sub-issue under #311 | scitadel-hub-deploy | H0; `microvm.nix` on anvil |
| H11 — on-prem packaging (NixOS module + OCI + Helm) | sub-issue under #311 | scitadel-hub-deploy | H0; stage 0 hub running |
| H12 — standalone `scitadel-mcp` (crates.io + PyPI wheel + npm) | sub-issue under #311 | scitadel | H3 |
| H13 — TDM opt-out pipeline (robots.txt + TDMRep + `noai` + ai.txt, re-checked on every re-ingest) | sub-issue under #311 | hub | §12; before any commercial hosted tier |
| H14 — `scitadel-hub-fixtures` CC0 slice (if open question 2 confirms) | sub-issue under #311 | scitadel | H0, H5 |

Upstream: vig-os/tessera#778 waves:
- W0: #758, #628;
- W1: #654, #666, #668;
- W2: #768 → #769, #775;
- W3: #776;
- the gap issues filed for the hub:
  - W1: tessera#780 (declared keys and keyed references, work_id →
    product index), tessera#782 (pipeline and per-row provenance,
    external inputs not held);
  - W2: tessera#781 (keyed delta products);
  - tessera#779 (media-typed binary columns for figure bytes).

tessera#777 (publish `.tsra` on Hugging Face) is **reframed to
optional / selective** by decision 19: the walled corpus is the
product. The only use that remains is the CC0
`scitadel-hub-fixtures` slice, if open question 2 confirms it.

## Alternatives considered

- **DuckDB as the query engine.** Rejected: a C++ dependency and a
  second engine next to tessera's DataFusion provider. It would be
  a fine standalone choice, but nothing here needs it.
- **A graph database (Neo4j, Kùzu or a Rust successor) as the source of
  truth.** Rejected for now: Neo4j is a JVM server. Kùzu is no longer
  developed, and its Rust successors are months old. Edge tables plus
  an adjacency cache cover citation and CPC graphs. A trait keeps a
  Cypher layer pluggable.
- **Lance, Qdrant, pgvector or Milvus as the vector store.** Rejected:
  a second source of truth beside tessera, with its own format and
  versioning. tessera#769 keeps vectors in Vortex, with ANN structures
  as rebuildable caches.
- **One workspace with the hub behind cargo features.** Rejected: the
  hub's dependencies would enter every client build, CI run and
  `cargo install`, and the two release cadences would couple.
- **A full live OpenAlex mirror on the free key.** Rejected: the free
  key cannot ask what changed, re-fetching everything works against
  OpenAlex's model, and the data exceeds anvil's disk.
- **Member+ for everyone.** Not rejected; it is the optional path in
  §5 for institutions that have it. It isn't a requirement, because
  most labs won't.
- **PatentRef as the long-term patent source.** Rejected: a free beta
  from an undisclosed operator, with no service commitment, a 1,000
  requests/month free tier and no pre-grant publications. It is the
  stopgap.
- **Publish the processed corpus on Hugging Face.** Rejected. The
  processed corpus and the GPU hours that produced it are not given
  away: a curated public dataset is no longer a moat (anyone with a
  weekend and a budget can rebuild one from CC0/CC-BY/PD sources), and
  bulk copies only dilute the running service the hub sells —
  freshness, licence-class gating, extraction-cascade curation, System
  One ranking, per-row provenance. The one possible exception is a
  tiny CC0 conformance/fixtures slice (`scitadel-hub-fixtures`),
  carried as open question 2 in §16.
- **A Parquet projection for the HF viewer and the `datasets`
  library.** Rejected as a corollary of the walled corpus: there is
  no public HF dataset to project to. A client that wants Parquet runs
  `scitadel detach` against the hub (§12) and re-exports locally.
- **TEI, JATS or Typst as the storage format.** Rejected: XML trees and
  typesetting source are not queryable at corpus scale, compress
  worse than typed columns, and Typst conversion is lossy. They are
  ingest sources (TEI, JATS) or render views (Typst) instead (§9).
- **One `.tsra` per paper.** Rejected for the hub: per-product overhead
  exceeds a paper's text, tiny blocks compress badly, and corpus
  queries would open millions of objects. Kept for client-side
  `detach` (§12).
- **PDFs inside tessera products.** Rejected: ~1.5 MB per paper of
  mostly fonts and rasters would dominate every shard. Originals live
  in the artefact store, under a retention policy (§12).
- **Sending every paper to a frontier model.** Rejected: cost, and
  licences forbid sending unlicensed or paywalled text to an external
  API. The cascade (§10) spends each tier only on what the cheaper
  tiers could not settle.
- **USPTO ODP API as the patent source.** Rejected for search: it
  serves application metadata, not full-text search, and allows only
  one concurrent call per key. It remains useful for single-record
  status lookups.

## Consequences

- A second repo to maintain, with its own CI on the fleet runner.
- scitadel gains a protocol crate and a `hub` adapter. Behaviour
  without a hub configured is unchanged.
- scitadel-hub depends on tessera before its 1.0. Every product is
  rebuildable, so a format change costs a rebuild, not data.
- The hub inherits tessera's open issues:
  - tessera#628 (strings about 3× larger than Parquet) is the biggest
    size risk;
  - tessera#654 (whole-table materialisation) blocks queries at
    corpus scale;
  - tessera#758 (fork pin) silently drops determinism fixes for
    downstream dependents.
- anvil's 2 TB drive caps the hub's stage-0 scope at subsets plus
  patents; the warm and cold tiers of §8 absorb the overflow.
- The walled corpus means `scitadel-hub-deploy` is the only on-prem
  distribution path (§1). An admin can't skip the commercial grant by
  cloning a public NixOS module; the owner accepts the friction as a
  licensing / support signal, over the product-angle and license-spike
  reviewers' preference for a public `Apache-2.0` deploy repo.
- Keeping the corpus walled forecloses the "give the data away to earn
  citations" distribution path. Downstream adoption now depends on
  the client, the protocol and the hosted tiers earning it.
- The hub holds licensed content. Licence classes, entitlement checks
  and the T4 gate are security-relevant code and need tests, not
  conventions.
- **CC-BY-NC and CC-BY-ND are excluded from every paid hosted tier**
  (§12). Embeddings are treated as adapted material to be safe; the
  per-row licence column is the enforcement surface, not a convention.
- **A TDM opt-out pipeline is on the critical path for the hosted
  tier** (§12, H13). robots.txt / TDMRep / `noai` / ai.txt must be
  re-checked on every re-ingest; a hosted tier that skips this cannot
  rely on EU DSM Art. 4.
- **A DPIA plus an Art. 14 notice are pre-launch gates** for the
  first hosted user.
- **Trademark clearance is on the critical path** before any public
  branded release: `scitadel` registered at IGE Switzerland in
  Nice classes 9 + 42 after a TMview + IGE + USPTO + WIPO search
  ("Citadel" is crowded).
- T3 (sage) and T4 (the Opus API) cost GPU time and money. Budget caps
  and queueing bound them; the random-sample QC sets a floor on that
  spend.
- The golden set is a permanent asset to curate: its private half
  needs a private bucket and fleet runners.
- Serving on CPU means an index that needs more RAM than the server
  box has must fall back to an on-disk ANN (IVF-PQ or DiskANN-style),
  paying tens of ms per probe rather than the RAM path's ms.
  The 51 GB raw-vectors estimate is a ceiling, not a floor.
- Client-side query embedding couples the client to the hub's
  embedding-model identity, version and output dims. The
  `capabilities` advertisement (§7) plus the hub's explicit mismatch
  rejection keeps that coupling safe, but a client that cannot run
  the current embedder (thin clients, old builds, a window after a
  hub re-embedding) has to fall back to hub-side embedding and pays
  the hub's CPU for it.

## Open questions for the owner

1. **Protocol crate home:** ~~this repo (default) or the hub repo?~~
   **Resolved (2026-11-08): this repo, under `MIT OR Apache-2.0`**
   (decision 18, §1, §16). Rationale: an AGPL proto crate would
   contaminate clients and third-party integrators.
2. **`scitadel-hub-fixtures` CC0 slice:** publish a tiny CC0
   conformance / fixtures slice (`scitadel-hub-fixtures`) as the one
   exception to the walled-corpus rule, so external CI, bug reports
   and conformance tests are reproducible? (Replaces the old "HF
   org" question, superseded by decision 19.) If yes, size and host
   (crate-embedded, or a small CC0 HF dataset owned by `vig-os`)?
3. **Patent scope:** grants only (default), or add pre-grant
   publications (more data, more disk)?
4. **OpenAlex membership:** does ETH (or the lab) hold an OpenAlex
   Member or Member+ plan? If so, §5's paid path applies.
5. **Disk:** if a larger OpenAlex subset is wanted, more storage on
   anvil (or another host) is needed. Is that in scope for
   g-fleet#1363?
6. **Default subset:** which subscriptions should the first lab mirror
   carry (topics, the lab's institution, members' libraries)?
7. **Licence reading:** does ETH Library confirm that free-to-read
   works without a licence may be cached and mined lab-internally
   under CopA Art. 24d, and which publisher licences allow sharing
   among ETH members through a hub?
8. **Retention:** keep the last 2 bases (default), or more?
9. **T3 model:** which Qwen vision-language model size fits sage? sage's
   specs are unverified.
10. **T3/T4 budgets:** the daily caps for sage time and Opus API spend.
11. **Rerank placement (§14):** on the hub's `gpu_submit` queue by
    default (co-located, but shares the 5090 queue with inference
    and ingest), on sage (lower-contention GPU, one more hop), or
    either, chosen by cost and queue depth? The `reranked: false`
    fallback covers each choice either way.
12. **Crossref TDM fields (§15):** does Crossref still expose both
    the `license` URL and
    `link[intended-application=text-mining]` entries on current
    records, for every CC-BY/CC0 publisher we'd harvest through
    (MDPI, Frontiers, Copernicus, IOP, APS, Springer Nature OA,
    IEEE Access, ACS AuthorChoice CC-BY)? And which of those
    publishers add TDM-specific terms on top of the CC licence
    that we must honour per-publisher in the paced buckets?
13. **Research-analytics layer (§18):** when does the
    DataFusion-over-works-authorships-institutions-funders-citations
    tier (a WoS Research-Intelligence equivalent for research
    offices) move from "designed later" to "scheduled"? No design
    is proposed here; only the shape.
