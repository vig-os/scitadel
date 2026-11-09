---
type: issue
state: open
created: 2026-11-09T04:23:52Z
updated: 2026-11-09T04:23:52Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/340
comments: 0
labels: feature, effort:medium, priority:high, semver:minor
assignees: none
milestone: 0.10.0
projects: none
parent: 316
children: none
synced: 2026-11-09T08:50:49.114Z
---

# [Issue 340]: [feat(proto): create the scitadel-hub-proto crate in this repo (MIT OR Apache-2.0) for the client/hub protocol](https://github.com/vig-os/scitadel/issues/340)

## Problem

The client/hub wire protocol (H3, #316) needs a crate that both sides depend on: the scitadel client and [nerdmachines/scitadel-hub](https://github.com/nerdmachines/scitadel-hub). ADR-008 §16 (PR #317) places it in this repo under `MIT OR Apache-2.0`, so the client never depends on the hub repo, and AGPL never reaches the client tree. The crate doesn't exist yet, and #316 can't start without it.

## Proposal

- New workspace crate `crates/scitadel-hub-proto` (working name), `MIT OR Apache-2.0`, published with the release train.
- Request and response types for `search` (text or a declared query vector; mode bm25 | vector | hybrid), `get`, `neighbours`, `coverage` and `capabilities` (embedding model id, version, dims, Matryoshka truncation, analyzer, rerankers). Results are snippet-only, with `answered_by`, `reranked` and licence tags per item.
- Depends only on `scitadel-core` domain types (`Paper`, identifiers, `UntrustedText`). No tessera, DataFusion or tantivy.
- Versioned wire format with an explicit protocol version in `capabilities`. Unknown fields are tolerated, so either side can be upgraded first.
- Spec text under the Community Specification License 1.0 and conformance vectors under CC0 (#333, #331).

## Acceptance

- [ ] The crate builds in the workspace with no new heavy dependencies; `cargo deny` stays clean.
- [ ] serde round-trip tests per message; a golden JSON fixture per message is shared with #331's conformance kit.
- [ ] An embedding-model mismatch is a typed error, never a silent re-embed (ADR-008 §14).
- [ ] #316 builds against it.

Refs #316, #313, #311, #333, #331.

