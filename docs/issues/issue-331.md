---
type: issue
state: open
created: 2026-11-09T03:22:06Z
updated: 2026-11-09T03:22:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/331
comments: 0
labels: feature, effort:medium, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:51.783Z
---

# [Issue 331]: [feat(hub): H19 — scitadel-hub-fixtures CC0 slice and protocol conformance kit](https://github.com/vig-os/scitadel/issues/331)

Part of #311. Design: ADR-008 (PR #317), §13, §16; open question 2 (whether to publish the slice).

## Motivation
The corpus is walled, but integrators, contributors and CI need a reproducible slice to test against. It is also needed for a conformance kit that other hubs or clients can run against the protocol.

## Scope
- `scitadel-hub-fixtures`: a tiny CC0-only slice (OpenAlex metadata plus a handful of CC0 documents through the full document model). It carries no processed full text of non-CC0 items.
- A conformance kit: protocol test vectors (CC0-1.0) plus a runner that checks a hub or client against the spec (Community Specification License 1.0, §16).
- **Gated on the owner's answer to open question 2**: crate-embedded only, or also published as a small CC0 dataset.

## Acceptance
- CI of scitadel and scitadel-hub runs the conformance kit against the fixtures.
- The slice's licence manifest proves every row is CC0.

## Depends on
H0 (#313), H5 (#319).

