# ADR-004 — sentence-id normalization for the annotation resolver

- Status: accepted
- Date: 2026-04-19
- Driver: #96 (multi-selector resolver completion in 0.4.0)

## Context

The annotation anchor (#49 / `crates/scitadel-core/src/models/annotation.rs`)
carries a `sentence_id: Option<String>` field — a stable identifier for
the sentence containing the quoted passage. The resolver uses it as a
last-resort selector before declaring the anchor an orphan. Without a
written normalization spec, two correct implementations of "hash the
sentence" can disagree on whitespace, case, or ligature handling and
silently mis-anchor.

This ADR pins the spec so the writer (annotation creation) and reader
(`resolve_anchor` step 4) cannot diverge.

## Decision

`sentence_id(s: &str) -> String` is defined as:

1. **NFKC normalize** the input (`unicode_normalization::nfkc`).
   Compatibility decomposition + canonical composition. This folds
   ligatures (e.g. `ﬁ` U+FB01 → `fi`), full-width forms, and other
   compatibility-equivalent codepoints into their canonical
   representation.
2. **Lowercase** via `char::to_lowercase` (full Unicode lowercasing,
   not ASCII-only — handles German `ẞ` → `ß`, Turkish `İ` → `i̇`,
   etc.).
3. **Collapse whitespace**: every run of `char::is_whitespace`
   (ASCII space, tab, newline, NBSP, en-space, etc.) becomes a single
   ASCII `' '`. Trim leading and trailing whitespace.
4. **SHA1** the resulting bytes, encode as lowercase hex (40 chars).

Two sentences that differ only in case, whitespace presentation, or
ligature form will hash to the same value. Substantive edits — a
changed word, a reordered clause, a different number — will hash
differently and the resolver will fall through to `Orphan` for that
selector.

## Sentence boundaries

The current resolver splits paper text on `.`, `!`, `?` and treats each
run between terminators as a sentence (after trimming). This is
deliberately simple — good enough for paper bodies and abstracts that
don't contain abbreviations like "Dr.", "et al.", "i.e.". Proper ICU
sentence segmentation is a follow-up if mis-segmentation becomes a
real-world problem; switching the boundary algorithm without changing
`normalize_sentence` will not change existing `sentence_id` values
because the function operates on whatever sentence string is handed to
it.

## Out of scope

- **Stemming** — would conflate different words that share a stem
  (e.g. "ran" / "running") and cause sentence collisions.
- **Diacritic stripping** — preserves meaning across European
  languages where it would not (`über` ≠ `uber`).
- **Punctuation stripping** — same rationale; "the model failed." vs.
  "the model failed?" should hash differently.

## Consequences

- One canonical implementation in `scitadel-core`
  (`models::annotation::sentence_id`); both writers and the resolver
  call it. Cross-crate impls are forbidden.
- Stored `sentence_id` values are stable across scitadel versions as
  long as this ADR holds. Changes to normalization semantics require a
  superseding ADR and a one-shot migration that re-hashes anchors
  against the new spec.
- The `unicode-normalization` and `sha1` workspace dependencies are
  load-bearing — a major version bump that changes NFKC tables or
  SHA1 output (the latter shouldn't happen, but) requires a re-hash
  migration.
