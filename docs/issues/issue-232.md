---
type: issue
state: closed
created: 2026-09-28T18:01:33Z
updated: 2026-09-29T00:08:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/232
comments: 1
labels: bug, area:workflow
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:23.707Z
---

# [Issue 232]: [MCP tools print short IDs but require full IDs: create_question/list_* output can't be passed to add_search_terms, save_assessment, get_papers](https://github.com/vig-os/scitadel/issues/232)

## Summary

The IDs that scitadel's MCP tools *print* are 8-character short IDs, `Id::short()`. Most tools that *accept* an ID do an exact lookup, so an agent that copies an ID from a tool response gets `Question '<id>' not found.` / `Search '<id>' not found.`, even though `list_questions` / `list_searches` just showed that ID. A prefix resolver, `resolve_question_id` in `crates/scitadel-mcp/src/tools.rs`, already exists, but only the bib-shortlist tools use it.

## Repro (0.8.0, MCP)

```
create_question(text=…, description=…)     → "Question created: f523f2ca …"   # short id only
list_questions()                           → "f523f2ca  2026-09-28 12:18  "…""
add_search_terms(question_id="f523f2ca")   → Error: Question 'f523f2ca' not found.
list_searches()                            → "a9458223  …"
get_papers(search_id="a9458223")           → Error: Search 'a9458223' not found.
```

The full IDs exist in the DB (e.g. `research_questions.id = f523f2cabe1e4f06b98f96c8c6744f7c`), and calls succeed with them. Nothing in the create/list output reveals the full ID, though. `search` returns the full `search_id` in its JSON, which is the only reason searches work in practice.

**Impact:** in a real session, one of two agents could not persist any assessments or search terms (every `save_assessment` / `add_search_terms` failed), while the other happened to obtain a full ID and succeeded.

## Where

Exact-match `get_question(qid)` sits in the `search` (question_id), `add_search_terms`, `get_rubric`, `prepare_assessment`, `save_assessment` and `prepare_batch_assessments` tools (`tools.rs` ~L40, 359, 397, 477, 520, 960). The same pattern applies to `get_papers` / search-id lookups. `create_question_tool` and `list_questions_tool` print `id.short()`.

## Suggested fix

- Route every ID-accepting tool through a prefix resolver: generalise `resolve_question_id` to questions, searches and papers, with an ambiguity error.
- Have `create_question` (and the list tools, at least in structured output) return the full ID.
- Add an MCP test: `create_question` → take the printed ID → `add_search_terms` / `save_assessment` round-trip.

---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 12:08 AM_

Fixed in #237: every ID-accepting MCP tool now takes a full id or a unique prefix of at least 4 characters (`[A-Za-z0-9-]` only), and create and list output now print full ids. An ambiguous prefix returns an error listing both candidates.

