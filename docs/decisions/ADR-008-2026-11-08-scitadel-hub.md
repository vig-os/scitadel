# ADR-008 — scitadel-hub: a lab corpus server (repo split, tessera storage, sync strategy, retrieval)

**Status**: Proposed.
**Date**: 2026-11-08
**Supersedes**: —
**Context**: #311 (epic), #313–#316 (H0–H3), #268 (PatentsView retirement), #246 (System One ranking), vig-os/tessera#778, gerchowl/g-fleet#1121

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
corpus* and how clients use it. The two do not overlap: the hub holds
metadata, abstracts, patent full text (public domain) and derived
indexes; it does not fetch publisher full text.

## Decision

1. **A separate repo, `vig-os/scitadel-hub`**, holds the corpus
   server. scitadel stays the client: CLI, MCP, TUI, GUI, source
   adapters, acquisition (ADR-007), review workflow and local SQLite.
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
   and published on Hugging Face as `.tsra` only. PatentRef remains a stopgap in the
   client adapter, not a dependency of the hub.
8. **The hub runs on fleet infrastructure:** the S3-compatible
   research-data store (versitygw, g-fleet#1121), a POSIX working copy
   under `/srv/anvil/projects/<project>/`, and GPU work only as queued
   batch jobs.

## 1 · Repo split

| | scitadel (client) | scitadel-hub (server) |
|---|---|---|
| Contents | CLI, MCP, TUI, GUI; source adapters; acquisition (ADR-007); review workflow; local SQLite | Ingest (snapshot, bulk files, deltas); sync jobs; index builds; query service |
| Dependencies | Light: reqwest, rusqlite, ratatui, … | Heavy: tessera, Vortex, DataFusion, tantivy, an embedding runtime |
| Runs on | A laptop | anvil |
| CI / release | Fast CI, devkit release train | Slower nix CI on the fleet runner; GPU work as jobs, not in CI; own cadence |

**The contract between them:**

- `scitadel-core` (published) supplies domain types: `Paper`,
  identifiers, `UntrustedText`.
- A protocol crate (working name `scitadel-hub-proto`) holds the
  request and response types (§7). **Default: it lives in this repo**
  and ships with the release train, so the client never depends on the
  hub repo. Open question 1 asks whether it should move to the hub.
- Repo settings come from vig-os/org-config (otterdog), as for this
  repo.

The repo is created only after this ADR is Accepted (#313).

## 2 · Data model on tessera

- **Products.** One product per (corpus, release):
  - `openalex-works-subset@2026-10` (quarterly base);
  - `openalex-works-delta@2026-11-08` (daily delta);
  - `uspto-grants@2026-W45` (weekly delta);
  - a quarterly patent re-seal.

  Each product records its provenance: the upstream release, the
  filter that defined the subset, and the code version that built it.
- **Tables:** works, authorships, abstracts, patents, claims,
  description sections, CPC codes, and edge tables (§4). These are
  Vortex table blocks.
- **Add-only.** Nothing is rewritten in place. A delta product shadows
  rows of the base by ID; a periodic compaction writes a new base. This
  matches the fleet rule that existing data under `/srv/anvil` is never
  moved, rewritten or deleted.
- **Derived means rebuildable.** Every product can be rebuilt from
  upstream. That is why tessera's pre-1.0 format is acceptable here: a
  format change costs a rebuild, not data.
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
- **Publication.** A Hugging Face dataset under a `vig-os` org,
  shipped as **`.tsra` only**, with a dataset card (tessera#777).
  Consumers read columns straight from HF over HTTP range reads through
  tessera's object-store reader (tessera ADR-0034); Vortex gives
  better column access than Parquet. The card states the licences (PatentsView tables
  CC-BY-4.0 with attribution; USPTO text public domain in the US),
  the provenance (USPTO release, embedding model and version) and the
  build recipe. Size: 100–150 GB in total *(estimate)*, inside HF's
  free public storage.
- **Novelty:** nothing on Hugging Face combines US full text from 1976
  to now with embeddings and a search index. HUPD stops at 2018 and
  has no embeddings. This makes the dataset useful beyond the lab.
- **PatentRef stopgap.** scitadel's PatentsView adapter points at a
  configurable PatentSearch-compatible endpoint, defaulting to
  PatentRef (#268 thread). It is a third-party free beta with no
  service commitment. Once a hub with H1 exists, the hub answers
  patent searches and PatentRef is only the fallback.

## 7 · Client/hub protocol

| Call | Input | Output |
|---|---|---|
| `search` | query text, filters, mode (`bm25` / `vector` / `hybrid`), limit | Ranked candidates, each with a score and the mode that produced it |
| `get` | Up to 100 IDs | Records |
| `neighbours` | ID, edge kind, depth | Graph neighbours |
| `coverage` | — | The sources, subsets and product versions the hub holds |

- **Provenance on every response:** it names the product seal or
  version that produced it.
- **Untrusted text:** every text field arrives as `UntrustedText`
  (ADR-007 amendment, #287).
- **Fallback:** the client asks the hub only for what `coverage`
  covers, and falls through to the live API on a miss, an error, or a
  bounded timeout. Every result records `answered_by: hub | <source>`.
- **Transport:** HTTP+JSON first. The types do not assume it.

## 8 · Infrastructure

- **Store.** The S3-compatible research-data store on anvil (versitygw,
  gerchowl/g-fleet#1121) is the canonical store and the distribution
  point. A POSIX working copy under `/srv/anvil/projects/<project>/`
  serves DataFusion, tantivy and GPU jobs, which read local files much
  faster than ranged S3 reads.
- **Disk.** anvil's only documented drive is one 2 TB 990 PRO. Budget
  *(estimate)*:
  - patents: 100–150 GB;
  - OpenAlex subsets: 5–50 GB each;
  - indexes: ~1.5× the text they cover;
  - initial total: about 1 TB including working copies.

  A full OpenAlex mirror is ~770 GB as Parquet and an estimated ~2.5 TB
  sealed in tessera until tessera#628 is fixed, so it **does not fit**
  and is not a goal.
- **GPU.** The 5090 is shared and usually busy with inference. The hub
  uses it only for batch embedding through `gpu_submit`:
  - declared `vram` and `priority=low`;
  - chunked to the 30-minute default runtime, with checkpoints;
  - outputs written to new directories only.

  No index stays resident in VRAM; queries are served from CPU and
  NVMe.
- **Scoring.** System One scoring follows #246 (hosted Clef or local
  Kev). Where Kev runs is a deployment detail of #246, not of this ADR.

## Slices

| Slice | Issue | Where | Depends on |
|---|---|---|---|
| H0 — repo split + protocol crate home | #313 | both | This ADR Accepted |
| H1 — patent corpus + HF publication | #314 | hub | H0; tessera W0/W1 (internal Parquet staging first is fine), W2 for indexes, #777 for publication; g-fleet#1121 |
| H2 — OpenAlex lab mirror | #315 | hub | H0; tessera W0 (#628) and W1; g-fleet#1121 |
| H3 — `hub` source adapter + protocol | #316 | scitadel | H0; a mock hub for tests, H1/H2 for end-to-end |

Upstream: vig-os/tessera#778 waves:
- W0: #758, #628;
- W1: #654, #666, #668;
- W2: #768 → #769, #775, #777;
- W3: #776.

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
- **A Parquet projection for the HF viewer and the `datasets`
  library.** Deferred. Publishing `.tsra` only gives up the Hugging
  Face dataset viewer and `load_dataset()` compatibility. A Parquet
  projection can be added later if consumers ask for it, as a derived
  export of the sealed products.
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
- anvil's 2 TB drive caps the hub's scope at subsets plus patents.
- A public Hugging Face dataset under `vig-os` brings a maintenance
  promise: a dataset card, a weekly or quarterly cadence, and licence
  attribution.

## Open questions for the owner

1. **Protocol crate home:** this repo (default) or the hub repo?
2. **Hugging Face org:** `vig-os`, or a lab or ETH organisation?
3. **Patent scope:** grants only (default), or add pre-grant
   publications (more data, more disk)?
4. **OpenAlex membership:** does ETH (or the lab) hold an OpenAlex
   Member or Member+ plan? If so, §5's paid path applies.
5. **Disk:** if a larger OpenAlex subset is wanted, more storage on
   anvil (or another host) is needed. Is that in scope for
   g-fleet#1121?
6. **Default subset:** which subscriptions should the first lab mirror
   carry (topics, the lab's institution, members' libraries)?
