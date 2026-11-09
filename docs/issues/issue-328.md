---
type: issue
state: closed
created: 2026-11-09T03:21:56Z
updated: 2026-11-09T03:50:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/328
comments: 1
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:53.223Z
---

# [Issue 328]: [feat(deploy): H16 — on-prem packaging: NixOS module, OCI image, Helm chart (private deploy repo)](https://github.com/vig-os/scitadel/issues/328)

Part of #311. Design: ADR-008 (PR #317), §1 (topology), §8, §16. Target repo: `scitadel-hub-deploy` (private, not yet created).

## Motivation
Institutions run the hub on their own infrastructure. The owner decided the packaging lives in a private repo, as deliberate friction: on-prem comes with the commercial licence or a support contract. The AGPL hub source stays public.

## Scope
- A NixOS module `services.scitadel-hub` (options, secrets, storage backends, versitygw/S3, systemd hardening).
- An OCI image and a Helm chart, built reproducibly from the same Nix derivation.
- Model weights with restrictive terms (e.g. Marker) are fetched at first run via a signed manifest, never baked into images (§16).
- The fleet consumes the module pinned by tag and never patches it locally.

## Acceptance
- A clean NixOS VM boots a working hub from the module alone.
- The OCI image and Helm chart pass a smoke test against the same fixtures.
- The SBOM and third-party notices ship with every artefact.

## Depends on
H0 (#313); the stage-0 hub running.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 9, 2026 at 03:50 AM_

Moved to https://github.com/nerdmachines/scitadel-hub-deploy/issues/2, the private repo for this work (ADR-008 §1). Tracked there from now on.

