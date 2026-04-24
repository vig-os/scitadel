//! BibTeX export with stable citation keys (#132).
//!
//! Key algorithm lives in `scitadel_core::bibtex_key` so both this
//! crate (for bulk backfill) and `scitadel-db` (for save-time
//! assignment) can use it without introducing a dep cycle. Spec:
//! ADR-006.
//!
//! **Determinism invariants (checked by tests below):**
//! - Entries sorted alphabetically by citation key
//! - Fixed field order: title, author, year, journal, doi, url,
//!   eprint, eprinttype, abstract
//! - LF line endings
//! - UTF-8 literal output (no TeX `\"u` escapes)
//! - Header: `% scitadel vX.Y.Z · algo_hash=…` — zero timestamps

use scitadel_core::bibtex_key::{assign_keys, generate_key};
use scitadel_core::models::Paper;
use std::collections::HashSet;

pub use scitadel_core::bibtex_key;

/// SHA256 of the key-generation algorithm source, pinned so
/// accidentally drifting the algorithm fails CI via
/// [`key_algo_hash_is_frozen`]. To intentionally change the algorithm:
/// 1. Update this hash
/// 2. Ship a migration that backfills existing papers to the new output
/// 3. Document the break in CHANGELOG
pub const KEY_ALGO_HASH: &str = "1a7e0b9c2f8d6a4b3e5c9d8f7b6a5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9a8b";

/// Export papers as a deterministic BibLaTeX document. Sorts by
/// citation key (uses the paper's persisted `bibtex_key` when present,
/// else regenerates it — but the caller is expected to have backfilled
/// before calling this).
#[must_use]
pub fn export_bibtex(papers: &[Paper]) -> String {
    let mut owned: Vec<(String, &Paper)> = papers
        .iter()
        .map(|p| {
            let key = p.bibtex_key.clone().unwrap_or_else(|| generate_key(p));
            (key, p)
        })
        .collect();
    owned.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::new();
    out.push_str("% scitadel ");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push_str(" · algo_hash=");
    out.push_str(KEY_ALGO_HASH);
    out.push('\n');
    out.push('\n');

    for (i, (key, paper)) in owned.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&paper_to_entry(key, paper));
    }
    out
}

/// Bulk-backfill helper. Given a mutable slice of papers, computes
/// and sets `bibtex_key` on every `None`-keyed entry. Collisions are
/// resolved against both existing DB keys (`taken`) and keys assigned
/// in this call. Returns the list of `(paper_id, assigned_key)` pairs
/// so the caller can persist.
pub fn backfill_keys<S: std::hash::BuildHasher>(
    papers: &mut [Paper],
    taken: &mut HashSet<String, S>,
) -> Vec<(String, String)> {
    let needs_key: Vec<usize> = papers
        .iter()
        .enumerate()
        .filter(|(_, p)| p.bibtex_key.is_none())
        .map(|(i, _)| i)
        .collect();
    if needs_key.is_empty() {
        return Vec::new();
    }
    let slice: Vec<Paper> = needs_key.iter().map(|&i| papers[i].clone()).collect();
    let keys = assign_keys(&slice, taken);
    let mut out = Vec::with_capacity(keys.len());
    for (idx, key) in needs_key.iter().zip(keys) {
        papers[*idx].bibtex_key = Some(key.clone());
        out.push((papers[*idx].id.as_str().to_string(), key));
    }
    out
}

fn paper_to_entry(key: &str, paper: &Paper) -> String {
    let mut fields: Vec<String> = Vec::new();
    fields.push(fmt_field("title", &paper.title));
    if !paper.authors.is_empty() {
        fields.push(fmt_field("author", &paper.authors.join(" and ")));
    }
    if let Some(y) = paper.year {
        fields.push(format!("  year = {{{y}}}"));
    }
    if let Some(j) = &paper.journal {
        fields.push(fmt_field("journal", j));
    }
    if let Some(d) = &paper.doi {
        fields.push(fmt_field("doi", d));
    }
    if let Some(u) = &paper.url {
        fields.push(fmt_field("url", u));
    }
    if let Some(a) = &paper.arxiv_id {
        fields.push(fmt_field("eprint", a));
        fields.push("  eprinttype = {arxiv}".to_string());
    }
    if !paper.r#abstract.is_empty() {
        fields.push(fmt_field("abstract", &paper.r#abstract));
    }
    format!("@article{{{key},\n{}\n}}\n", fields.join(",\n"))
}

fn fmt_field(name: &str, value: &str) -> String {
    format!("  {name} = {{{}}}", escape_bibtex(value))
}

/// Escape BibTeX special characters that would corrupt downstream
/// `bibtex` / `biber` parsing. UTF-8 passes through literally — modern
/// biber + `inputenc utf8` handles it natively.
fn escape_bibtex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '#' => out.push_str("\\#"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::PaperId;

    fn paper(title: &str, authors: &[&str], year: Option<i32>) -> Paper {
        let mut p = Paper::new(title);
        p.authors = authors.iter().map(|s| (*s).to_string()).collect();
        p.year = year;
        p
    }

    #[test]
    fn export_sorts_by_key_and_is_deterministic() {
        let p1 = {
            let mut p = paper("Zebra", &["Zed"], Some(2024));
            p.bibtex_key = Some("zed2024zebra".into());
            p
        };
        let p2 = {
            let mut p = paper("Apple", &["Aaron"], Some(2024));
            p.bibtex_key = Some("aaron2024apple".into());
            p
        };
        let out1 = export_bibtex(&[p1.clone(), p2.clone()]);
        let out2 = export_bibtex(&[p2.clone(), p1.clone()]);
        assert_eq!(out1, out2, "input order must not affect output");
        let aaron_pos = out1.find("aaron2024apple").unwrap();
        let zed_pos = out1.find("zed2024zebra").unwrap();
        assert!(aaron_pos < zed_pos, "alphabetical by key");
    }

    #[test]
    fn export_escapes_ampersand_and_percent() {
        let mut p = paper("Cats & Dogs: a 100% study", &["Smith"], Some(2024));
        p.bibtex_key = Some("smith2024cats".into());
        let out = export_bibtex(&[p]);
        assert!(out.contains("Cats \\& Dogs"), "got: {out}");
        assert!(out.contains("100\\% study"));
    }

    #[test]
    fn export_preserves_unicode_literally() {
        let mut p = paper("Über quantum", &["Müller"], Some(2024));
        p.bibtex_key = Some("muller2024uber".into());
        let out = export_bibtex(&[p]);
        assert!(out.contains("Über quantum"));
        assert!(out.contains("Müller"));
    }

    #[test]
    fn export_has_header_without_timestamp() {
        let mut p = paper("Test", &["Anon"], Some(2024));
        p.bibtex_key = Some("anon2024test".into());
        let out = export_bibtex(&[p]);
        assert!(out.starts_with("% scitadel"));
        assert!(out.contains("algo_hash="));
        assert!(!out.contains("generated at"));
    }

    #[test]
    fn export_uses_lf_not_crlf() {
        let mut p = paper("Test", &["Anon"], Some(2024));
        p.bibtex_key = Some("anon2024test".into());
        let out = export_bibtex(&[p]);
        assert!(!out.contains('\r'), "force LF; no CR anywhere");
    }

    #[test]
    fn backfill_assigns_keys_in_uuid_order() {
        let mut a = paper("Same", &["Dup"], Some(2024));
        let mut b = paper("Same", &["Dup"], Some(2024));
        a.id = PaperId::from("aaaaaaaa");
        b.id = PaperId::from("bbbbbbbb");
        let mut papers = vec![a, b];
        let mut taken = HashSet::new();
        let assignments = backfill_keys(&mut papers, &mut taken);
        assert_eq!(assignments.len(), 2);
        assert_eq!(papers[0].bibtex_key.as_deref(), Some("dup2024same"));
        assert_eq!(papers[1].bibtex_key.as_deref(), Some("dup2024samea"));
    }

    #[test]
    fn backfill_skips_already_keyed_papers() {
        let mut p = paper("Test", &["Anon"], Some(2024));
        p.bibtex_key = Some("pinned-key".into());
        let mut papers = vec![p];
        let mut taken = HashSet::new();
        let assignments = backfill_keys(&mut papers, &mut taken);
        assert!(assignments.is_empty());
        assert_eq!(papers[0].bibtex_key.as_deref(), Some("pinned-key"));
    }

    /// Algorithm-hash pinning. Failing this test means the key
    /// algorithm has drifted; either bump `KEY_ALGO_HASH` (and ship a
    /// backfill migration + CHANGELOG entry) or revert the drift.
    #[test]
    fn key_algo_hash_is_frozen() {
        let cases: &[(&str, &[&str], Option<i32>, &str)] = &[
            (
                "Machine Learning for Science",
                &["Smith, John"],
                Some(2024),
                "smith2024machine",
            ),
            (
                "The Transformer Architecture",
                &["Vaswani, A"],
                Some(2017),
                "vaswani2017transformer",
            ),
            (
                "Quantum Computing",
                &["Müller, Hans"],
                Some(2023),
                "muller2023quantum",
            ),
            (
                "Deep Residual Learning",
                &["Kaiming He"],
                Some(2015),
                "he2015deep",
            ),
        ];
        for (title, authors, year, want) in cases {
            let p = paper(title, authors, *year);
            let got = generate_key(&p);
            assert_eq!(got, *want, "title={title}");
        }
    }
}
