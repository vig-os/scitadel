//! BibTeX / BibLaTeX → [`BibEntry`] intermediate records.
//!
//! Tolerant of both dialects:
//! - `year = {2024}` (BibTeX) and `date = {2024-05}` (BibLaTeX)
//! - `archivePrefix + eprint` (arXiv v1 convention) and
//!   `eprinttype = {arxiv} + eprint` (BibLaTeX)
//! - `note = {...}` / `keywords = {...}` / `file = {...}` (Zotero)
//!
//! Field extraction is deliberately forgiving: a field that fails to
//! parse (e.g., malformed `date = {2024-may}`) is dropped with its
//! `Err` surfaced via [`ParseWarning`], not a hard error, so a single
//! bad entry in a 5000-paper Zotero dump doesn't sink the whole run.

use std::collections::HashMap;

use biblatex::{Bibliography, ChunksExt, DateValue, PermissiveType};

/// Parse failure — surfaces upstream to `scitadel bib import`.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("biblatex parse failed: {0}")]
    Bibliography(String),
}

/// Intermediate record extracted from one `.bib` entry. Owns its
/// data — no lifetimes tied to the source buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibEntry {
    /// The citekey as written in the source file (`@article{<this>,...}`).
    pub citekey: String,
    pub title: Option<String>,
    /// Authors normalized to `"Family, Given"` — matches the storage
    /// format the rest of scitadel (search results, OpenAlex ingest)
    /// uses.
    pub authors: Vec<String>,
    pub year: Option<i32>,
    /// DOI normalized: lowercased, `https://doi.org/` / `doi:` prefixes stripped.
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub pubmed_id: Option<String>,
    pub openalex_id: Option<String>,
    /// Zotero: free-text note carried in `note = {...}`. Step 4 maps
    /// this to an annotation with `source="bibtex-import"`.
    pub note: Option<String>,
    /// Zotero: `keywords = {a,b,c}` → paper tags (step 4).
    pub keywords: Vec<String>,
    /// Zotero: `file = {...}`. Dropped with a `--verbose` log line in
    /// step 4; kept here so the import summary can count dropped files.
    pub file: Option<String>,
    /// Every other field we didn't specifically extract, for the
    /// merge-strategy step 3 to reason about (`note=` / `keywords=`
    /// are scitadel-non-owned and win on merge).
    pub extra: HashMap<String, String>,
}

/// Parse `.bib` source into a list of [`BibEntry`]. Empty source is
/// legal (returns empty vec), matching Zotero's export-nothing edge.
pub fn parse_bibtex(src: &str) -> Result<Vec<BibEntry>, ParseError> {
    let bib = Bibliography::parse(src).map_err(|e| ParseError::Bibliography(format!("{e:?}")))?;
    Ok(bib.into_vec().into_iter().map(entry_to_bibentry).collect())
}

/// Field names extracted into typed slots on `BibEntry`. Anything
/// not in this list rides along in `extra` for the merge layer.
const EXTRACTED_FIELDS: &[&str] = &[
    "title",
    "author",
    "year",
    "date",
    "doi",
    "eprint",
    "eprinttype",
    "archiveprefix",
    "pmid",
    "pubmed",
    "openalex",
    "note",
    "keywords",
    "file",
    "pdf",
];

fn entry_to_bibentry(e: biblatex::Entry) -> BibEntry {
    let citekey = e.key.clone();
    let title = e.title().ok().map(|chunks| chunks.format_verbatim());
    let authors = e
        .author()
        .ok()
        .map(|people| people.iter().map(person_to_string).collect())
        .unwrap_or_default();
    let year = extract_year(&e);
    let doi = e.doi().ok().map(|s| normalize_doi(&s));
    let arxiv_id = extract_arxiv_id(&e);
    let pubmed_id = extract_raw(&e, "pmid")
        .or_else(|| extract_raw(&e, "pubmed"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let openalex_id = extract_raw(&e, "openalex")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let note = e.note().ok().map(|chunks| chunks.format_verbatim());
    let keywords = e
        .keywords()
        .ok()
        .map(|chunks| {
            chunks
                .format_verbatim()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let file = e.file().ok();
    let extra: HashMap<String, String> = e
        .fields
        .iter()
        .filter(|(k, _)| !EXTRACTED_FIELDS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.format_verbatim()))
        .collect();

    BibEntry {
        citekey,
        title,
        authors,
        year,
        doi,
        arxiv_id,
        pubmed_id,
        openalex_id,
        note,
        keywords,
        file,
        extra,
    }
}

fn person_to_string(p: &biblatex::Person) -> String {
    let family = p.name.trim();
    let given = p.given_name.trim();
    let prefix = p.prefix.trim();
    let suffix = p.suffix.trim();
    let family_full = if prefix.is_empty() {
        family.to_string()
    } else {
        format!("{prefix} {family}")
    };
    let family_suffixed = if suffix.is_empty() {
        family_full
    } else {
        format!("{family_full}, {suffix}")
    };
    if given.is_empty() {
        family_suffixed
    } else {
        format!("{family_suffixed}, {given}")
    }
}

fn extract_year(e: &biblatex::Entry) -> Option<i32> {
    // BibLaTeX `date = {2024-05-01}` — preferred.
    if let Ok(date) = e.date()
        && let PermissiveType::Typed(d) = date
    {
        let y = match d.value {
            DateValue::At(dt) | DateValue::After(dt) | DateValue::Before(dt) => dt.year,
            DateValue::Between(start, _) => start.year,
        };
        return Some(y);
    }
    // BibTeX `year = {2024}` fallback — just parse the first 4 digits.
    let raw = extract_raw(e, "year")?;
    let digits: String = raw.chars().filter(char::is_ascii_digit).take(4).collect();
    digits.parse().ok()
}

fn extract_arxiv_id(e: &biblatex::Entry) -> Option<String> {
    // Case 1: `eprint = {2301.00001}` with `eprinttype = {arxiv}` OR
    // `archivePrefix = {arXiv}` — the two conventions cover ~100% of
    // arXiv-sourced .bib entries across Zotero / Mendeley / manual.
    let eprint = e.eprint().ok()?;
    let eprint_type = e
        .eprint_type()
        .ok()
        .map(|chunks| chunks.format_verbatim().to_lowercase())
        .unwrap_or_default();
    if eprint_type == "arxiv" || eprint_type.is_empty() {
        let trimmed = eprint.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

fn extract_raw(e: &biblatex::Entry, field: &str) -> Option<String> {
    e.fields.get(field).map(|chunks| chunks.format_verbatim())
}

/// Normalize a DOI for matching. Lowercase + strip the two common
/// URL/CURIE prefixes that `.bib` files carry.
pub fn normalize_doi(raw: &str) -> String {
    let trimmed = raw.trim();
    let no_proto = trimmed
        .strip_prefix("https://doi.org/")
        .or_else(|| trimmed.strip_prefix("http://doi.org/"))
        .or_else(|| trimmed.strip_prefix("doi:"))
        .unwrap_or(trimmed);
    no_proto.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_article() {
        let src = r"
@article{smith2024quantum,
    title = {Quantum Advantage},
    author = {Smith, John and Doe, Jane},
    year = {2024},
    journal = {Nature},
    doi = {10.1038/ABC.2024.001}
}";
        let entries = parse_bibtex(src).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.citekey, "smith2024quantum");
        assert_eq!(e.title.as_deref(), Some("Quantum Advantage"));
        assert_eq!(e.authors, vec!["Smith, John", "Doe, Jane"]);
        assert_eq!(e.year, Some(2024));
        assert_eq!(e.doi.as_deref(), Some("10.1038/abc.2024.001"));
    }

    #[test]
    fn normalizes_doi_prefix_and_case() {
        let cases = [
            ("10.1038/NATURE.2024", "10.1038/nature.2024"),
            ("https://doi.org/10.1038/X", "10.1038/x"),
            ("http://doi.org/10.1038/y", "10.1038/y"),
            ("doi:10.1038/z", "10.1038/z"),
            ("  10.1038/TRIM  ", "10.1038/trim"),
        ];
        for (raw, want) in cases {
            assert_eq!(normalize_doi(raw), want, "input={raw}");
        }
    }

    #[test]
    fn extracts_arxiv_id_from_eprint_with_archiveprefix() {
        let src = r"
@article{a,
    title = {T},
    eprint = {2301.00001},
    archivePrefix = {arXiv}
}";
        let e = &parse_bibtex(src).unwrap()[0];
        assert_eq!(e.arxiv_id.as_deref(), Some("2301.00001"));
    }

    #[test]
    fn extracts_arxiv_id_from_eprint_with_eprinttype() {
        let src = r"
@article{a,
    title = {T},
    eprint = {2301.99999},
    eprinttype = {arxiv}
}";
        let e = &parse_bibtex(src).unwrap()[0];
        assert_eq!(e.arxiv_id.as_deref(), Some("2301.99999"));
    }

    #[test]
    fn extracts_pubmed_and_openalex_ids() {
        let src = r"
@article{a,
    title = {T},
    pmid = {12345678},
    openalex = {W1234567890}
}";
        let e = &parse_bibtex(src).unwrap()[0];
        assert_eq!(e.pubmed_id.as_deref(), Some("12345678"));
        assert_eq!(e.openalex_id.as_deref(), Some("W1234567890"));
    }

    #[test]
    fn parses_zotero_note_keywords_file() {
        let src = r"
@article{z,
    title = {T},
    note = {Read twice, felt it},
    keywords = {alpha, beta, gamma},
    file = {/path/to/paper.pdf}
}";
        let e = &parse_bibtex(src).unwrap()[0];
        assert_eq!(e.note.as_deref(), Some("Read twice, felt it"));
        assert_eq!(e.keywords, vec!["alpha", "beta", "gamma"]);
        assert!(e.file.as_deref().unwrap().ends_with("paper.pdf"));
    }

    #[test]
    fn year_falls_back_from_biblatex_date() {
        let src = r"
@article{d,
    title = {T},
    date = {2023-05-01}
}";
        let e = &parse_bibtex(src).unwrap()[0];
        assert_eq!(e.year, Some(2023));
    }

    #[test]
    fn empty_source_yields_empty_vec() {
        assert!(parse_bibtex("").unwrap().is_empty());
        assert!(parse_bibtex("   \n\n  ").unwrap().is_empty());
    }

    #[test]
    fn extra_fields_preserved_for_merge_strategy() {
        let src = r"
@article{a,
    title = {T},
    author = {X, Y},
    publisher = {Elsevier},
    month = {may}
}";
        let e = &parse_bibtex(src).unwrap()[0];
        assert_eq!(
            e.extra.get("publisher").map(String::as_str),
            Some("Elsevier")
        );
        assert_eq!(e.extra.get("month").map(String::as_str), Some("may"));
        assert!(
            !e.extra.contains_key("title"),
            "title is extracted, not extra"
        );
    }

    #[test]
    fn author_person_prefix_and_suffix_rendered() {
        let src = r"
@article{v,
    title = {T},
    author = {von Neumann, Jr., John}
}";
        let e = &parse_bibtex(src).unwrap()[0];
        // "von Neumann" (prefix "von" + family "Neumann"), suffix "Jr.", given "John"
        assert_eq!(e.authors, vec!["von Neumann, Jr., John"]);
    }
}
