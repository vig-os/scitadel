//! `.bib` import pipeline (#134 iter 2).
//!
//! Parses BibTeX / BibLaTeX source into intermediate [`BibEntry`]
//! records and matches them against existing papers via the narrow
//! [`PaperLookup`] trait. Merge strategies and Zotero-compat
//! (note/keywords/file) land in subsequent commits.
//!
//! Lives in `scitadel-export` alongside the export direction
//! (`bibtex.rs`) because the parse+write-back loop for the round-trip
//! invariant is easier to reason about when both directions share a
//! crate. The matcher uses a trait rather than concrete DB types so
//! the test suite can exercise the cascade without a SQLite fixture.

pub mod matcher;
pub mod parse;

pub use matcher::{MatchOutcome, PaperLookup, match_entry};
pub use parse::{BibEntry, ParseError, parse_bibtex};
