---
type: issue
state: open
created: 2026-11-09T03:21:47Z
updated: 2026-11-09T03:21:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/325
comments: 0
labels: feature, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:54.640Z
---

# [Issue 325]: [feat(app): H13 — client app: Tauri 2 desktop + mobile and a web dual build on one Rust backend](https://github.com/vig-os/scitadel/issues/325)

Part of #311. Design: ADR-008 (PR #317), §17. Lives in scitadel.

## Motivation
The client must run offline-first on a personal machine and also reach hosted personal/lab/org spaces from a browser or phone. Today scitadel ships a CLI, a TUI, an MCP server and a GUI. One app surface across desktop, mobile and web avoids maintaining three UIs.

## Scope
- A Tauri 2 shell (macOS, Linux, Windows; iOS, Android) plus a web dual build of the same frontend.
- One Rust backend crate behind a typed transport (specta / tauri-specta). Tauri IPC in the app, HTTP against a hub in the web build. No business logic in the frontend.
- The structured-document reader (no PDFs; ADR-008 §9), annotations anchored per element, and the extraction-issue control of H12 (#324).
- Offline-first: the local SQLite store works with no hub. With a hub, the H3 (#316) adapter is tried first.
- `detach`: a user may pull full copies of their own tracked works. No bulk export of the processed corpus (decision 19).

## Acceptance
- The same frontend builds as a desktop app, a mobile app and a web bundle in CI.
- Typed bindings are generated, not hand-written, and CI fails if they drift.
- A review can be run end to end offline on desktop.

## Depends on
H0 (#313), H3 (#316).

