---
type: issue
state: open
created: 2026-11-09T03:22:03Z
updated: 2026-11-09T03:22:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/330
comments: 0
labels: feature, effort:medium, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:52.229Z
---

# [Issue 330]: [feat(hub): H18 — TDM opt-out pipeline (robots.txt, TDMRep, noai, ai.txt) for the commercial tier](https://github.com/vig-os/scitadel/issues/330)

Part of #311. Design: ADR-008 (PR #317), §12. Lives in scitadel-hub.

## Motivation
The EU DSM Art. 3 TDM exception covers research organisations, e.g. a library running the hub on-prem. A **commercial hosted tier** relies on Art. 4 only, which binds it to rights holders' machine-readable opt-outs. Swiss CopA Art. 24d does not permit commercial redistribution of mining results.

## Scope
- Record opt-out signals per source and per item: robots.txt, TDMRep (`tdm-reservation`), `noai` meta and headers, ai.txt.
- Re-check them on every re-ingest. An item that gains an opt-out is withdrawn from commercial tiers, keeping the provenance of the withdrawal.
- Tag every row with the legal basis it is held under (Art. 3 research, Art. 4 commercial, a licence such as CC0/CC-BY/PD).
- Gate: no commercial hosted tier serves anything without an Art. 4-clear row or an explicit licence.

## Acceptance
- Fixture sources with each opt-out kind are excluded from the commercial view and stay in the research view.
- A flipped opt-out between two ingests withdraws the item.

## Depends on
H4 (#318), H5 (#319). Required before any commercial hosted tier.

