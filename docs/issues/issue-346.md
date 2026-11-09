---
type: issue
state: open
created: 2026-11-09T06:21:24Z
updated: 2026-11-09T06:21:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/346
comments: 0
labels: bug, effort:small, priority:low, semver:patch
assignees: none
milestone: 0.9.0
projects: none
parent: none
children: none
synced: 2026-11-09T08:50:47.421Z
---

# [Issue 346]: [fix(reader): window-start escape snap, CSI-straddle test, long-word highlight hop — #338 follow-ups](https://github.com/vig-os/scitadel/issues/346)

Follow-ups from the final review of #338 (#307). None blocks; all are small.

1. **Snap the window start backwards** (`crates/scitadel-mcp/src/tools.rs:1747`). A caller-supplied `offset` that lands inside an OSC sequence makes the rest of the sequence, including a hyperlink URL, show as plain text in that window. No raw escape bytes survive. Offsets taken from `next_offset` are already safe. Fix: snap `start` back to the beginning of an escape range that contains it. Add a test with a mid-OSC offset.
2. **Test the CSI-straddle case for real** (`tools.rs:3175`). After the first snap, no boundary lands on the CSI, so that case is never exercised, although the comment claims it is. Choose `max_chars` so that a boundary does split a CSI sequence.
3. **Long first word in `line_of_highlight`** (`crates/scitadel-tui/src/views/reader.rs:521`). When the quote's first word is wider than the pane, the hop lands on the word's last row instead of its first. Cap the measured width at the pane width. The test at `reader.rs:1303` (`long_row > 0`) always passes because of the border, so assert the exact rendered row instead.

Refs #307, #338.

