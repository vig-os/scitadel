//! Loaders for `bib diff`: read a file path, sniff format, return
//! format-neutral [`Entry`] records.
//!
//! Lives next to [`crate::diff`] so the diff CLI / MCP tool can call a
//! single helper rather than re-implementing the BibTeX-or-JSON branch
//! at every call site. Content-sniffing is the source of truth: a
//! `.bib` file containing valid JSON is detected as JSON. Extension is
//! only consulted when content-sniffing is ambiguous (which it never
//! actually is — JSON either parses or it doesn't), so extension is
//! effectively cosmetic in this helper. Kept here purely so callers
//! can also accept content via [`load_entries_from_str`].

use std::path::Path;

use crate::diff::Entry;
use crate::import::parse::parse_bibtex;

/// Detected format of a bibliography file. Returned by
/// [`detect_format_from_str`] so callers can branch (e.g. log a warn
/// when extension and content disagree).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BibFormat {
    Bibtex,
    CslJson,
}

/// Load a file and return its entries in the format-neutral shape.
/// Errors are bubbled up verbatim with file-path context so the
/// caller can present a useful error.
pub fn load_entries_from_path(path: &Path) -> Result<(Vec<Entry>, BibFormat), String> {
    let bytes =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    load_entries_from_str(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Same as [`load_entries_from_path`] but takes the bytes directly.
/// The CLI uses this for the `--question-id` form (where the "before"
/// side is a fresh DB snapshot, not a file).
pub fn load_entries_from_str(src: &str) -> Result<(Vec<Entry>, BibFormat), String> {
    match detect_format_from_str(src) {
        BibFormat::CslJson => {
            let v: serde_json::Value =
                serde_json::from_str(src).map_err(|e| format!("not valid JSON: {e}"))?;
            let arr = v
                .as_array()
                .ok_or_else(|| "CSL-JSON top-level must be an array".to_string())?;
            let entries: Vec<Entry> = arr.iter().filter_map(Entry::from_csl_value).collect();
            Ok((entries, BibFormat::CslJson))
        }
        BibFormat::Bibtex => {
            let parsed = parse_bibtex(src).map_err(|e| format!("BibTeX parse failed: {e}"))?;
            let entries: Vec<Entry> = parsed.iter().map(Entry::from_bib_entry).collect();
            Ok((entries, BibFormat::Bibtex))
        }
    }
}

/// Sniff format. Strategy: try parsing as JSON first (fast: `serde_json`
/// will reject within bytes if the leading non-whitespace is `@`). If
/// JSON parse fails, fall back to BibTeX. Note that an empty BibTeX
/// file (zero entries) is legal, and an empty string parses as neither
/// JSON nor BibTeX cleanly; we treat empty-or-whitespace-only as
/// BibTeX so an empty `.bib` round-trips correctly.
#[must_use]
pub fn detect_format_from_str(src: &str) -> BibFormat {
    if src.trim().is_empty() {
        return BibFormat::Bibtex;
    }
    // Quick eyeball: JSON docs start with `[` or `{` after whitespace.
    // `@article{...}` (BibTeX) starts with `@`. Don't even bother
    // calling serde_json if the first non-whitespace byte rules JSON
    // out — saves allocation on large `.bib` files.
    let first = src.trim_start().as_bytes().first().copied();
    if matches!(first, Some(b'[' | b'{')) && serde_json::from_str::<serde_json::Value>(src).is_ok()
    {
        return BibFormat::CslJson;
    }
    BibFormat::Bibtex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_csl_json_array() {
        let s = r#"[{"id":"a","type":"article-journal","title":"T"}]"#;
        assert_eq!(detect_format_from_str(s), BibFormat::CslJson);
    }

    #[test]
    fn detects_bibtex_canonical() {
        let s = r"@article{a, title = {T}, year = {2024}}";
        assert_eq!(detect_format_from_str(s), BibFormat::Bibtex);
    }

    #[test]
    fn empty_input_defaults_to_bibtex() {
        assert_eq!(detect_format_from_str(""), BibFormat::Bibtex);
        assert_eq!(detect_format_from_str("   \n  "), BibFormat::Bibtex);
    }

    #[test]
    fn malformed_json_falls_back_to_bibtex() {
        // Looks JSON-ish but isn't — should fall through to bibtex parse.
        let s = "[ this is not json {} ]";
        assert_eq!(detect_format_from_str(s), BibFormat::Bibtex);
    }

    #[test]
    fn content_sniff_overrides_extension_intent() {
        // `.bib` extension by intent, but content is valid JSON.
        // Caller should still treat as JSON.
        let s = r#"[{"id":"x","type":"article-journal","title":"T"}]"#;
        let (entries, fmt) = load_entries_from_str(s).unwrap();
        assert_eq!(fmt, BibFormat::CslJson);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].citekey, "x");
    }

    #[test]
    fn loads_bibtex_entries_into_neutral_shape() {
        let s = r"
@article{smith2024,
  title = {T},
  author = {Smith, J.},
  year = {2024},
  doi = {10.1/ABC}
}";
        let (entries, fmt) = load_entries_from_str(s).unwrap();
        assert_eq!(fmt, BibFormat::Bibtex);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].citekey, "smith2024");
        assert_eq!(entries[0].title.as_deref(), Some("T"));
        // DOI normalized.
        assert_eq!(entries[0].doi.as_deref(), Some("10.1/abc"));
    }

    #[test]
    fn loads_csl_json_entries_into_neutral_shape() {
        let s = r#"[
            {"id":"smith2024","type":"article-journal","title":"T",
             "author":[{"family":"Smith","given":"J."}],
             "issued":{"date-parts":[[2024]]},
             "DOI":"10.1/ABC"}
        ]"#;
        let (entries, fmt) = load_entries_from_str(s).unwrap();
        assert_eq!(fmt, BibFormat::CslJson);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].citekey, "smith2024");
        assert_eq!(entries[0].title.as_deref(), Some("T"));
        // CSL DOI is also normalized via the lift helper.
        assert_eq!(entries[0].doi.as_deref(), Some("10.1/abc"));
        assert_eq!(entries[0].year, Some(2024));
    }

    #[test]
    fn rejects_csl_top_level_object() {
        // A single CSL entry as a bare object is not the canonical
        // shape (must be an array). We reject explicitly.
        let s = r#"{"id":"x","type":"article-journal","title":"T"}"#;
        let err = load_entries_from_str(s).unwrap_err();
        assert!(err.contains("must be an array"), "got: {err}");
    }
}
