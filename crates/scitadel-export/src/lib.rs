pub mod bibtex;
pub mod csl_json;
pub mod csv_export;
pub mod diff;
pub mod diff_format;
pub mod diff_input;
pub mod import;
pub mod json_export;
pub mod sidecar;
pub mod slug;
pub mod snapshot;

pub use bibtex::{export_bibtex, export_bibtex_with_tags};
pub use csl_json::{export_csl_json, export_csl_json_with_tags};
pub use csv_export::export_csv;
pub use diff::{BibDiff, ChangedEntry, Entry, FieldChange, diff_entries};
pub use diff_format::{render_json as render_diff_json, render_text as render_diff_text};
pub use diff_input::{
    BibFormat, detect_format_from_str, load_entries_from_path, load_entries_from_str,
};
pub use import::{
    ALIAS_SOURCE, AliasRecord, BibEntry, MatchOutcome, MatchStrategy, MergeAction, MergeOutcome,
    MergeStrategy, PaperLookup, SideEffects, compute_side_effects, match_entry, paper_from_bib,
    parse_bibtex, resolve as resolve_merge,
};
pub use json_export::export_json;
pub use sidecar::{BibLockfile, content_hash, shortlist_hash};
pub use slug::{FALLBACK_SLUG, MAX_SLUG_LEN, slugify};
pub use snapshot::{
    SIDECAR_SUFFIX, SnapshotFormat, SnapshotOutcome, default_filename_for_format, sidecar_path_for,
    write_snapshot,
};
