---
type: issue
state: closed
created: 2026-11-09T03:21:50Z
updated: 2026-11-09T03:50:32Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/326
comments: 1
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:54.154Z
---

# [Issue 326]: [feat(cloud): H14 — accounts, tiers and metering (grounded-query value metric)](https://github.com/vig-os/scitadel/issues/326)

Part of #311. Design: ADR-008 (PR #317), §1 (topology), §17. Target repo: `scitadel-cloud` (private, not yet created). Tracked here until the extraction trigger of §1 is met.

## Motivation
Hosted personal, lab and org spaces need accounts, plans and usage metering. The value metric is the **grounded query**: one MCP or REST call answered with cited, licence-tagged records. Seats and organisation-size bands are packaging only.

## Scope
- Accounts, organisations and groups, mapped onto the layered hubs (global → org → group → user).
- Plans and entitlements as data (tables), not code paths.
- Metering of grounded queries per tenant: idempotent, auditable, exportable.
- Never metered: solo local and offline use, reading full text the user already holds, annotations, citation export, and `capabilities`/`coverage` discovery calls.
- Licence floor: CC-BY-NC and CC-BY-ND items are never served in a paid tier (§12). A response says when items were skipped, and how many.

## Acceptance
- Metering is a pure function of the request log, so it can be replayed.
- Entitlement checks live in one place, and the hub reads them via the protocol.
- No prices in code; prices are configuration.

## Depends on
H13, H0 (#313); the extraction trigger of ADR-008 §1.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 9, 2026 at 03:50 AM_

Moved to https://github.com/nerdmachines/scitadel-cloud/issues/1, the private repo for this work (ADR-008 §1). Tracked there from now on.

