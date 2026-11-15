---
type: issue
state: closed
created: 2026-11-09T03:21:53Z
updated: 2026-11-09T03:50:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/327
comments: 1
labels: feature, effort:medium, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:53.723Z
---

# [Issue 327]: [feat(deploy): H15 — tenant simulation on microVMs (stage 0)](https://github.com/vig-os/scitadel/issues/327)

Part of #311. Design: ADR-008 (PR #317), §8 (stage 0). Target repo: `scitadel-hub-deploy` (private, not yet created).

## Motivation
Before any hosted tenant exists, the multi-tenant shape (isolation, per-tenant config, upgrades, backups) must be exercised on owned hardware.

## Scope
- N simulated tenants (a personal, a lab and an org hub) as microVMs on anvil (`microvm.nix`), each running the hub module.
- Per-tenant secrets (a separate sops file), storage quotas and an S3 prefix on the warm tier (gerchowl/g-fleet#1363).
- Read-through caching between layers (global → org → group → user).
- Scripted upgrade, rollback, and restore from the cold tier (Storage Box via restic).

## Acceptance
- Tenants can't read each other's data. A test proves it.
- A hub upgrade rolls across tenants and can be rolled back.
- A restore from cold storage is rehearsed and documented.

## Depends on
H0 (#313); the stage-0 hub running; g-fleet#1363.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 9, 2026 at 03:50 AM_

Moved to https://github.com/nerdmachines/scitadel-hub-deploy/issues/1, the private repo for this work (ADR-008 §1). Tracked there from now on.

