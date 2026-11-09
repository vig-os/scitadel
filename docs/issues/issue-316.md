---
type: issue
state: open
created: 2026-11-08T11:46:57Z
updated: 2026-11-09T04:56:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/316
comments: 1
labels: feature, effort:medium, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: 340
synced: 2026-11-09T08:50:58.272Z
---

# [Issue 316]: [feat(adapters): H3 — hub source adapter and client/hub protocol, falling through to the live APIs](https://github.com/vig-os/scitadel/issues/316)

Part of #311. Design: ADR-008 (Proposed), PR #317. Lives in this repo.

## Motivation

A scitadel client needs one switch to use a lab hub: search and metadata are answered from the hub's local corpus when it has the answer, and from the live APIs when it doesn't. The client must not take on the hub's dependencies (tessera, DataFusion, tantivy), and an agent using scitadel over MCP must not be able to tell the difference except through a reported `answered_by` field.

## Proposal

- A **`hub` source adapter** in `scitadel-adapters`, registered like any other source, configured by URL and a credential stored the way other sources store theirs (`scitadel auth login hub`). When configured, it is tried before the live APIs for the sources the hub declares it covers (OpenAlex works, USPTO patents).
- **Protocol crate `scitadel-hub-proto`**, in **this repo** under MIT OR Apache-2.0 (decided in ADR-008 §16 / #313), published with the release train so the client never depends on the AGPL hub repo. The spec is under the Community Specification License 1.0, and the conformance vectors are CC0. Request/response types for
  - `search` (query text **or** a client-supplied query vector, filters, mode: bm25 | vector | hybrid, limit) → ranked candidates with scores and the retrieval mode that produced each;
  - `get` (IDs, up to 100 per call) → records;
  - `neighbours` (ID, edge kind, depth) → citation/patent graph neighbours;
  - `coverage` → which sources, subsets and snapshot/delta versions the hub holds (so a client knows when to fall through);
  - `capabilities` (per index) → embedding model id, version, output dims, Matryoshka truncation applied, BM25 analyzer configuration, and the available rerankers. Clients call this to decide whether to send a query vector or fall back to text.

  Responses carry the product seal or version that produced them (provenance), and every text field arrives as untrusted content (`UntrustedText`, ADR-007 amendment / #287).
- **Query shape.** `search` accepts a client-supplied query vector OR text. When a client supplies a vector, it declares the embedding model id, version and dims. The hub **rejects a mismatch** against the target index with an explicit error (never embeds silently under the wrong model). Hub-side embedding (text in, hub embeds) is the fallback for thin clients and for the window after an index re-embedding. See ADR-008 §14.
- **Response shape.** Results are **snippet-only**: IDs, scores, snippets and provenance; bytes flow only on `open` or `detach` (ADR-008 §12). Every result carries `answered_by: hub | <source>`; every search response also carries a `reranked: bool` flag — if the hub's rerank queue is busy it returns unreranked results with `reranked: false` rather than block the search (graceful fallback, ADR-008 §14).
- **Fallback rule:** the hub answers when its `coverage` covers the request; otherwise the client calls the live API. Every result records `answered_by`.
- **Offline routing.** When a detached local subset (ADR-008 §12 `detach`) covers the query's topic, the adapter routes the search locally and reports `answered_by: local-subset:<id>`. The remote hub is tried only when no local subset covers it. BM25 and ANN random access require the subset on local NVMe; remote `.tsra` over HTTP range reads suits `get`-by-ID and column scans, not interactive search.
- Transport: HTTP+JSON first (simple, debuggable, works through the fleet's networking); the protocol types do not assume it.
- MCP and TUI show which source answered. This adapter is the **MCP grounding path**: the hub's positioning (ADR-008 §17) is grounding agents against hallucination with citable, provenance-carrying answers, so `answered_by` and the provenance fields are part of the agent-facing contract, not decoration.

## Acceptance

- [ ] With no hub configured, behaviour is unchanged (existing tests pass untouched).
- [ ] With a hub configured, a search in a covered subset is answered by the hub; one outside it falls through to the live API; both report `answered_by`.
- [ ] Hub down or slow → falls through within a bounded timeout, with a notice, not an error.
- [ ] Protocol types round-trip (serde tests), and text fields are untrusted on arrival.
- [ ] A contract test against a recorded hub response (in the spirit of #268).
- [ ] `capabilities` returns, per index: embedding model id + version + dims + Matryoshka truncation, BM25 analyzer, available rerankers.
- [ ] `search` accepts a client-supplied query vector tagged with the model id/version/dims; a mismatch against the target index returns an explicit protocol error (not a silent hub-side re-embed).
- [ ] `search` accepts text and the hub embeds server-side; the adapter uses this path when the client cannot run the current embedder.
- [ ] `search` responses carry `reranked: bool`; when the hub's rerank queue is busy or unavailable, results come back with `reranked: false` rather than erroring.
- [ ] Every result carries `answered_by: hub | <source> | local-subset:<id>`; MCP and TUI surface it.
- [ ] Result records are snippet-only (ID, score, snippet, provenance); full bytes only come through `open` / `detach`.
- [ ] A detached local subset covering the query's topic is preferred over the remote hub; absence of a matching subset routes to the hub; both paths are tested end-to-end.

## Dependencies

- Not blocked on H0 (#313): the protocol crate lives here. A hub to talk to (H1 or H2, in nerdmachines/scitadel-hub) is needed for the end-to-end tests; the adapter itself can land first against a mock server.

## Open question (owner)

**Where the query is embedded by default.** ADR-008 §14 says client-side, with hub-side as the fallback. The product-angle review (2026-11-08) recommends the inverse: the hub embeds by default, client-supplied vectors become an explicit flag, and offline / detached-subset mode stays client-side. Its reasons: thin web and mobile clients and MCP agents have no local embedder, and a hub model upgrade shouldn't force coordinated client releases. The cost is the hub's query-time embedding compute, against ADR-008's CPU-serving model. Both paths and the mismatch-reject stay in the acceptance criteria either way. Only the default is undecided, and it is recorded in ADR-008 once the owner rules.


---

# [Comment #1]() by [gerchowl]()

_Posted on November 9, 2026 at 04:56 AM_

Amended for the 2026-11-08 owner decisions (walled corpus, nerdmachines repo home, licence-gated phase 1, NC/ND excluded from paid tiers, EU DSM Art. 4 opt-out). Source: triage-matrix Cluster E; ADR-008 batch on #317.

