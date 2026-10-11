---
type: issue
state: open
created: 2026-10-10T12:38:04Z
updated: 2026-10-10T12:38:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/307
comments: 0
labels: bug, effort:medium, priority:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-11T08:00:31.861Z
---

# [Issue 307]: [fix(reader): two uncapped body render paths — the TUI reader cannot scroll, and build_annotated_paper returns the whole document](https://github.com/vig-os/scitadel/issues/307)

## Problem

#306 built the body envelope (`UntrustedBody`) and wired `Paper.r#abstract` and
`Paper.full_text`. In doing so it found **two uncapped body render paths**, both
pre-dating the envelope and neither a security fault — an uncapped body now
reaches a terminal *neutralised* — but both are rendering-quality bugs.

### 1. The TUI reader has no scrolling

`crates/scitadel-tui/src/views/reader.rs` renders the whole `full_text` into one
`Paragraph` with `Wrap` and no scroll offset. A 40 kB body is written into a frame
buffer sized to the terminal, so a reader shows the top of the document and
nothing else — there is no way to reach the rest.

### 2. `build_annotated_paper` returns the whole body

`crates/scitadel-mcp/src/tools.rs` `build_annotated_paper` returns the entire
`full_text` with no `max_chars` to honour. That is larger than `read_paper`'s
20 000-character default, so an agent calling the annotated variant gets a
different — and much larger — document than the plain one, by accident rather
than by design.

## What the envelope already guarantees

Neither of these can deliver an escape sequence or a control character to a
screen unescaped. That is what #287 asked for and it is implemented and tested.
These are about how much of a document a person can actually reach.

## Proposal

1. **Reader scroll.** `Frame`/`List`/`Paragraph` with a native scroll offset is
   the straightforward route, keyed to the up/down and page keys. Note the reader
   already has highlight and search state, so the offset has to interact with
   `root_at` — a scroll that desynchronises from a match highlight would be worse
   than no scroll.
2. **`build_annotated_paper` cap.** Take an explicit `max_chars` defaulting to
   `read_paper`'s, and say in the return that the body was truncated, the way
   `read_paper` already does. Two tools returning different amounts of the same
   document without saying so is the #261 overclaim shape.

## Acceptance

- [ ] A body longer than the viewport is scrollable in the TUI reader, and a
      search match below the fold is reachable.
- [ ] `build_annotated_paper` and `read_paper` return the same amount of the same
      document for the same arguments, or each says what it did.

## Also worth noting

`CandidatePaper.r#abstract` in `scitadel-adapters` is still a plain `String`. It
is the pre-dedup transport, not a `Paper` field, so the envelope deliberately
stops at the model boundary. Making the write side strongly typed end to end is
the related follow-up, and it is separate from this one.

Refs: #287, #306

