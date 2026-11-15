---
type: issue
state: open
created: 2026-11-08T11:46:53Z
updated: 2026-11-09T04:55:58Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/313
comments: 2
labels: chore, effort:small, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:59.644Z
---

# [Issue 313]: [chore(repo): H0 — split off nerdmachines/scitadel-hub once ADR-008 is accepted](https://github.com/vig-os/scitadel/issues/313)

Part of #311. Design: ADR-008 (Proposed), PR #317.

## Motivation

The lab corpus server needs heavy, slow-building dependencies (tessera, Vortex, DataFusion, tantivy, an embedding runtime). It runs on a server (anvil), with GPU batch jobs and its own release cadence. scitadel itself is a laptop client (CLI, MCP, TUI, GUI) with a fast CI and the devkit release train. Putting the hub into this workspace behind cargo features would push those dependencies into every client build, CI run and `cargo install`, and couple two release cycles that have nothing to do with each other.

## Proposal

The owner decided the repo topology on 2026-11-08 and the repo home on 2026-11-09 (ADR-008 §16):

| Repo | Visibility | Licence | Holds |
|---|---|---|---|
| `vig-os/scitadel` (this repo) | public | MIT OR Apache-2.0 | the client, plus the protocol crate `scitadel-hub-proto` |
| [`nerdmachines/scitadel-hub`](https://github.com/nerdmachines/scitadel-hub) | public | `AGPL-3.0-only OR LicenseRef-scitadel-Commercial` | the hub server source |
| `nerdmachines/scitadel-hub-deploy` | **private** | proprietary | NixOS module, OCI images, Helm chart. It is deliberate friction for other admins: on-prem comes via the commercial licence or support. |
| `nerdmachines/scitadel-cloud` | **private** | proprietary | hosted-offering and business docs |

The hub repos live under `nerdmachines`, not `vig-os`, so vig-os/org-config otterdog doesn't apply to them. Issues can't move across owners, so hub-side work is filed directly in the nerdmachines repos.

- **Shared contract.** The hub depends on published `scitadel-core` for the domain models (`Paper`, identifiers, `UntrustedText`). The client/hub wire protocol lives in **`scitadel-hub-proto`, in this repo, under MIT OR Apache-2.0**. It is published with the release train, so the client never depends on the AGPL repo. The protocol spec is under the Community Specification License 1.0, and the conformance vectors are CC0.
- **What stays here:** source adapters, acquisition (ADR-007), the review workflow, local SQLite, the `hub` source adapter (H3, #316) and the protocol crate.
- **What moves to the hub:** nothing exists yet, so nothing moves. The hub starts with its V-model plan (nerdmachines/scitadel-hub milestone M0), then H1 and H2.
- **CI.** The public hub repo uses GitHub Actions for docs and trace gates. GPU work is submitted as jobs, never run in CI.

## Acceptance

- [x] `nerdmachines/scitadel-hub` exists (default branch `dev`) with the dual-licence files, a README and CONTRIBUTING.md.
- [x] `nerdmachines/scitadel-hub-deploy` and `nerdmachines/scitadel-cloud` exist and are private.
- [ ] ADR-008 is Accepted.
- [ ] The hub's trace gate CI is green on `dev` (nerdmachines/scitadel-hub#2).
- [ ] `scitadel-hub-proto` exists in this repo and is published (or path/git-pinned), so that H3 can build against it.
- [ ] The README in both public repos states the split and links ADR-008.

## Dependencies

- ADR-008 accepted.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 9, 2026 at 03:51 AM_

The repos exist as scaffolds (licence, README, CONTRIBUTING), created 2026-11-09 under the owner's `nerdmachines` org. That supersedes `vig-os/scitadel-hub` in this issue's title:
- https://github.com/nerdmachines/scitadel-hub (public, `AGPL-3.0-only OR LicenseRef-scitadel-Commercial`);
- nerdmachines/scitadel-hub-deploy (private);
- nerdmachines/scitadel-cloud (private).

Code still lands only after ADR-008 is Accepted. The protocol crate stays in vig-os/scitadel under MIT/Apache.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 9, 2026 at 04:55 AM_

Amended for the 2026-11-08 owner decisions (walled corpus, nerdmachines repo home, licence-gated phase 1, NC/ND excluded from paid tiers, EU DSM Art. 4 opt-out). Source: triage-matrix Cluster E; ADR-008 batch on #317.

