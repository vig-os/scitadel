# ADR-006 — Stable BibTeX Citation Key Algorithm

**Status**: Accepted
**Date**: 2026-04-21
**Supersedes**: —
**Context**: #132 (bib foundation)

## Decision

Papers persist a stable `bibtex_key` assigned on first encounter and
**never recomputed**. The algorithm is Better-BibTeX-style:
`{lastname}{year}{firstword}`, ASCII-folded via NFKD + diacritic strip,
lowercase, with `a`/`b`/`c`/…/`z`/`aa`/`ab`/… collision suffixes
tiebroken by paper UUID lexicographic order (lowest UUID wins base
key).

## Algorithm

1. **Lastname**: first author's last name, ASCII-folded, lowercased,
   alphabetic characters only. "Last, First" comma form takes the
   pre-comma token; "First Last" space form takes the final
   whitespace-separated token. `Müller, Hans` → `muller`. Empty when
   no authors.

2. **Year**: `paper.year` as a 4-digit string. Empty when absent.

3. **Firstword**: first whitespace-separated word of the title that
   (a) is at least 3 characters after ASCII-folding + alphanumeric
   filtering and (b) is not a stopword. Stopwords:
   `a an the on of for is and to in at by`. "The Transformer
   Architecture" → `transformer`.

4. **Base key**: concatenation `{lastname}{year}{firstword}`. When all
   three are empty, fall back to `paper-{short-uuid}`.

5. **Disambiguation**: if the base key collides with an existing key,
   append `a`, else `b`, … up through `z`, `aa`, `ab`, …. Tiebreaker
   for *which* paper gets the base key when two papers share the same
   metadata: lexicographic order of paper UUID — lowest wins. Keys are
   persisted, so a later paper with an older UUID does not retroactively
   steal the base.

## Freeze contract

Once a paper has a `bibtex_key`, it **must never change**. The key is
a name, not a description — `\cite{muller2024quantum}` in a user's
Typst/LaTeX draft must resolve tomorrow regardless of upstream
metadata churn (OpenAlex corrects an author name, a title gets
edited). The rendered bibliography reflects current metadata; the key
stays.

**Escape hatch** (shipping in #134): `scitadel bib rekey <paper-id>`
regenerates or explicitly sets a key, printing the old→new mapping so
users can `sed` their drafts.

## Determinism

The algorithm is a pure function of `(authors, year, title, paper_id)`
with:

- **No DB access** — tests can exercise it against synthetic `Paper`s.
- **No environment / locale** — byte-order comparison, not `std::cmp`
  with user locale.
- **No randomness** — collision suffixes are deterministic given the
  `taken` set's contents.
- **NFC ingest + NFKD fold** — `Müller` (NFC) and `Müller` (NFD) both
  fold to `muller`.

## Algorithm-hash pinning

`scitadel-export::bibtex::KEY_ALGO_HASH` is a SHA256 of the
algorithm's source code, checked against a golden fixture of curated
(input, expected_key) pairs in `key_algo_hash_is_frozen`. Changing
the algorithm bumps the hash; the golden-fixture test then fails
loudly, forcing an explicit migration + CHANGELOG entry.

To intentionally change the algorithm:

1. Update `KEY_ALGO_HASH` to the new hash
2. Update the golden-fixture expected values
3. Ship a migration that backfills existing papers to the new output
4. Document the break in `CHANGELOG.md`

## Alternatives considered

- **Hash-based keys** (`smith2024a3f`). Unreadable in `\cite{}`.
  Rejected by the ecosystem reviewer: "defeat the entire point of
  human-authored citations."
- **Monotonic integer suffix** (`smith2024`, `smith2024_2`). Conflicts
  with BibLaTeX's own disambiguation conventions. Rejected.
- **Freeze-on-first-ever-export instead of freeze-on-first-assignment**.
  Drops the guarantee that the key exists as soon as the paper does —
  every `get_paper` call would need to check and possibly assign.
  Rejected for determinism.
- **First-written wins tiebreaker** instead of lowest-UUID. Not
  deterministic across machines importing the same DB (clock skew).
  Rejected.

## References

- Better BibTeX for Zotero — https://retorque.re/zotero-better-bibtex/citing/
- Algorithm source: `crates/scitadel-core/src/bibtex_key.rs`
- Golden fixture: `key_algo_hash_is_frozen` in `crates/scitadel-export/src/bibtex.rs`
- Migration: `crates/scitadel-db/migrations/009_bibtex_keys.sql`
- Backfill hook: `Database::backfill_bibtex_keys` in `crates/scitadel-db/src/sqlite/mod.rs`
