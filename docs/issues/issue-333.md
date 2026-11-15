---
type: issue
state: open
created: 2026-11-09T03:22:13Z
updated: 2026-11-09T03:22:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/333
comments: 0
labels: chore, effort:small, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:50.856Z
---

# [Issue 333]: [chore(legal): licensing and trademark hygiene per ADR-008 §16](https://github.com/vig-os/scitadel/issues/333)

Part of #311. Design: ADR-008 §16 (PR #317). Not legal advice; the review is recorded in ADR-008 §16.

## Checklist
- [ ] Client tree: a `cargo-about` NOTICE, a `cargo-cyclonedx` SBOM per release, and a `cargo-deny` allowlist with an explicit copyleft ban (so the Tauri and app-store builds stay clean).
- [ ] `CONTRIBUTING.md`: solo author for now; an Apache-style ICLA from the first external PR. DCO alone can't carry the hub's dual licence.
- [ ] Hub: `AGPL-3.0-only OR LicenseRef-scitadel-Commercial`, with written AGPL §7 additional permissions for client/cloud interop and a short AGPL §13 interpretation note.
- [ ] Protocol crate stays `MIT OR Apache-2.0`; spec under the Community Specification License 1.0; conformance vectors CC0-1.0.
- [ ] Model-weight register: licence and terms per model used in the cascade (Qwen3-VL, Grobid, Docling, Marker), re-checked on every model bump.
- [ ] Trademark (owner action): a clearance search (TMview, IGE, USPTO, WIPO), then an IGE filing in classes 9 and 42.

