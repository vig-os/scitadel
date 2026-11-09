---
type: issue
state: open
created: 2026-11-09T04:23:54Z
updated: 2026-11-09T04:23:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/341
comments: 0
labels: feature, effort:medium, priority:medium, semver:minor
assignees: none
milestone: none
projects: none
parent: 324
children: none
synced: 2026-11-09T08:50:48.608Z
---

# [Issue 341]: [feat(reader): client-side extraction-issue reporting — per-element report action in TUI/app reader and an MCP tool, routed via the hub (#324)](https://github.com/vig-os/scitadel/issues/341)

## Problem

The hub serves no PDFs, so the structured document *is* what a reader sees, and readers are the extraction-QC sensors (H12, #324). #324 designs the report flow and its licence-based routing on the hub side, in [nerdmachines/scitadel-hub](https://github.com/nerdmachines/scitadel-hub). The client surfaces that raise a report don't exist yet: the TUI reader, the GUI/app reader (#325) and the MCP.

## Proposal

- **Reader control.** A per-element "report issue" action in the TUI reader, and in the app reader as specified in #325. It offers failure chips (misordered, table shifted, equation dropped, figure missing, wrong text) and an optional inline correction.
- **Pre-attached context.** The element id, the work id, the extractor and renderer versions, the hub version and the per-row provenance (tessera#782). The user never re-types what the client already knows.
- **MCP path.** A `report_extraction_issue` tool, so an agent that finds a broken element while grounding can flag it, using the same payload.
- **Routing.** The client posts to the hub endpoint from #324, and the hub routes by licence through squelch. With no hub configured, it falls back to a prefilled GitHub/squelch form that cites only CC-licensed fixtures.
- **Privacy.** No query text or annotations are attached unless the user opts in per report.

## Acceptance

- [ ] One action from the TUI reader produces a complete report payload, tested against a fixture element.
- [ ] The MCP tool emits the same payload, and agent-supplied text is wrapped as untrusted (#306, #308).
- [ ] The offline fallback is a prefilled form; nothing is sent without the user confirming.

Refs #324, #325, #329, #311.

