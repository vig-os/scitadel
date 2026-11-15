---
type: issue
state: open
created: 2026-11-09T03:49:59Z
updated: 2026-11-09T03:49:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/335
comments: 0
labels: feature, effort:large, priority:low, semver:minor
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-09T08:50:49.666Z
---

# [Issue 335]: [feat(bib): Zotero library migration — collections, tags, notes and annotations, attachments as identity-only](https://github.com/vig-os/scitadel/issues/335)

## Problem

Researchers with an existing Zotero library cannot move to scitadel without losing structure. Bib import (#134, #162) takes a .bib export, which drops collections, most notes and all PDF annotations. The "replace Zotero when pointed at a hub" story needs a real adoption path.

## Proposal

Import a Zotero library, read from either the local `zotero.sqlite` (opened read-only) or the Zotero Web API, into scitadel:

- **Items** matched with the same order as bib import (DOI, arXiv, PubMed, OpenAlex, then exact title + year), merging per the #134 strategies.
- **Collections** mapped to scitadel collections or shortlists, keeping hierarchy.
- **Tags** mapped to paper tags.
- **Notes** mapped to annotations, with provenance (`source`, Zotero item key) recorded.
- **PDF annotations** mapped to W3C selectors where possible (text quote and position selectors, building on #49 and #158); those that cannot be anchored are kept as unanchored notes with the page recorded.
- **Attachments** recorded by identity and hash only. The bytes are fetched from a hub or detached store and never copied by default (ADR-008 section 12).

This is the adoption path for the "replace Zotero when pointed at a hub" story. It builds on bib import (#134, #162) and the annotations work (#49, #158).

## Acceptance

- [ ] Read-only import from `zotero.sqlite` (including a locked or live database copy) and from the Web API.
- [ ] Items, collections, tags and notes round-trip into scitadel with idempotent re-import.
- [ ] PDF annotations become W3C-selector annotations where anchorable; a report lists the ones that were not.
- [ ] Attachments are stored as identity + hash records; no file bytes are copied unless explicitly requested.
- [ ] Per-item import summary and a dry-run mode.
- [ ] Docs and tests with a fixture Zotero library.

