---
type: issue
state: open
created: 2026-10-04T20:46:33Z
updated: 2026-10-04T20:46:33Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/287
comments: 0
labels: effort:medium, security, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T07:53:08.848Z
---

# [Issue 287]: [security(reader): read_paper renders attacker-controlled text with no untrusted-content envelope — blocks any release](https://github.com/vig-os/scitadel/issues/287)

## Problem

ADR-007 §1 requires an **untrusted-content envelope** for anything read out of a fetched or dropped-in document. It does not exist, and #253's S2 made the exposure materially worse by adding new ways for arbitrary bytes to enter the library.

A document's title is **attacker-controlled text**, and it is read and rendered:

| source | field | who controls it |
|---|---|---|
| any PDF | `/Title` in the info dictionary | whoever produced the PDF |
| any landing page | `<meta name="citation_title">` | the publisher, or anyone who can influence the page |
| JATS/HTML | `<article-title>` | same |

`identity.rs` and `magic.rs` now *parse* all three — that work is correct, and it is also what proves the fields are reachable and untrusted. The parsed strings flow into `artefacts.title` and are handed to the reader with no marker saying they are untrusted input.

## Why this is a release blocker rather than a hardening item

The reader is where a human decides whether a document is the one they asked for. A title is the cheapest thing an attacker can set, and a reader that renders fetched metadata as if it were our own is exactly the confused-deputy shape:

- a **display** string can carry terminal escape sequences, and this repo's TUI is a terminal application — so a crafted `/Title` can move the cursor, rewrite a status line, or repaint the status column;
- the same string reaching an **MCP tool return** lands in an agent's context, where it is indistinguishable from text scitadel produced;
- it reaches a **human deciding whether to trust a document**, which is the decision the acquisition ladder exists to support.

Nothing needs to be compromised. `scan` and `attach` now accept a file a person dropped on disk, and any fetched artefact's metadata is publisher-supplied. Both are ordinary inputs, not attacks.

## What ADR-007 §1 asks for

Mark the boundary once, at the point text enters the system, and make it survive to the reader:

1. **A type, not a convention.** Something like `UntrustedText` / a `Provenance::{Ours, PublisherSupplied, DroppedInFile}` marker on the field, so a consumer cannot accidentally treat publisher text as ours. A doc comment saying "be careful" is not a boundary.
2. **Neutralise at the edges.** Terminal escapes stripped or escaped on render; length capped; control characters removed. Never passed through unescaped to a terminal or into a tool return.
3. **Distinguish in the UI.** A reader showing a publisher-supplied title should be able to say so, so a human can weigh it — this is also what makes the identity check's "both titles shown" actionable rather than decorative.
4. **Say it in the tool return.** An agent needs to know a string came from a fetched document. It is not the agent's job to guess.

## Acceptance

- [ ] Publisher-supplied text cannot reach a terminal, a tool return, or a log unescaped.
- [ ] A test asserts a `/Title` or `citation_title` containing terminal escape sequences is neutralised, on both the TUI render path and the MCP return path.
- [ ] The provenance of a displayed title is recoverable, so "is this ours or the publisher's?" has an answer at the call site.
- [ ] The ADR's envelope requirement is either implemented or explicitly descoped by a maintainer decision — not left as a line in an accepted ADR that nothing implements.

## Notes from implementation

- `crates/scitadel-adapters/src/identity.rs` and `magic.rs` already parse `/Title` and `citation_title`; reusing their extraction is cheaper than a second parser and keeps one answer to "what does this document call itself".
- `read_paper` and the TUI's reader are the two consumers. `find_cached_file` and the TUI's state column are also still on the legacy dual-write (see #253's S2e) and should move in the same pass.

Refs: #253, ADR-007 §1.
