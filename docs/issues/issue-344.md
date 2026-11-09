---
type: issue
state: open
created: 2026-11-09T05:35:22Z
updated: 2026-11-09T05:35:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/344
comments: 0
labels: bug, effort:small, priority:medium, semver:patch
assignees: none
milestone: 0.9.0
projects: none
parent: none
children: none
synced: 2026-11-09T08:50:48.171Z
---

# [Issue 344]: [fix(tui): reader highlights drift — char_range (stored-text coords) applied to the neutralised rendered() body](https://github.com/vig-os/scitadel/issues/344)

## Problem

The TUI reader places annotation and match highlights by `char_range`, but it slices the **neutralised** `rendered()` body. The neutraliser (#306) strips zero-width, bidi and control characters, so every character it removes before an anchor shifts that highlight left by one. Since PR #338, the MCP side windows over stored-text coordinates, consistent with `char_range`. The TUI still mixes the two coordinate spaces.

Found by the re-review of #338. It predates that PR.

## Proposal

- Map stored-text `char_range` to rendered positions through an offset map built once while neutralising, e.g. `UntrustedBody::rendered_with_map() -> (String, OffsetMap)`. Never slice rendered text with stored coordinates.
- Use the same map in any other renderer that highlights by `char_range` (GUI or app reader, #325).

## Acceptance

- [ ] A body with zero-width, bidi and control characters before an anchor highlights exactly the anchored quote in the TUI. Tested with a ratatui TestBackend.
- [ ] One mapping helper; no ad-hoc offset arithmetic in views.

Refs #307, #306, #338.

