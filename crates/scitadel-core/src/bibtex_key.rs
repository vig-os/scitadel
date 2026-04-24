//! Better-BibTeX-style citation key algorithm (#132). Lives in
//! scitadel-core so both `scitadel-db` (for save-time assignment) and
//! `scitadel-export` (for bulk backfill + formatting) can depend on it
//! without cycles.
//!
//! Spec is pinned in ADR-006. See also
//! `scitadel-export::bibtex::KEY_ALGO_HASH` — a SHA256 of the
//! pertinent source here, checked by a test that fails loudly when
//! the algorithm drifts.

use crate::models::Paper;
use std::collections::HashSet;

/// Stopwords stripped from the first word of the title. Keep conservative —
/// adding entries here breaks keys previously assigned to papers whose title
/// started with that word.
const TITLE_STOPWORDS: &[&str] = &[
    "a", "an", "the", "on", "of", "for", "is", "and", "to", "in", "at", "by",
];

/// Algorithm-assigned base key with no collision disambiguator.
/// Deterministic on `(authors[0], year, title, paper_id)` alone —
/// no DB, no environment, no locale.
#[must_use]
pub fn generate_key(paper: &Paper) -> String {
    let author = first_author_lastname(paper);
    let year = paper.year.map(|y| y.to_string()).unwrap_or_default();
    let word = first_title_word(paper);

    let key = format!("{author}{year}{word}");
    if key.is_empty() {
        format!("paper-{}", paper.id.short())
    } else {
        key
    }
}

fn first_author_lastname(paper: &Paper) -> String {
    let Some(raw) = paper.authors.first() else {
        return String::new();
    };
    let comma_split: Vec<&str> = raw.split(',').map(str::trim).collect();
    let name = if comma_split.len() >= 2 {
        comma_split[0]
    } else {
        raw.split_whitespace().last().unwrap_or(raw)
    };
    let folded = ascii_fold(name).to_lowercase();
    folded.chars().filter(char::is_ascii_alphabetic).collect()
}

fn first_title_word(paper: &Paper) -> String {
    for raw in paper.title.split_whitespace() {
        let folded: String = ascii_fold(raw)
            .to_lowercase()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        if folded.len() < 3 {
            continue;
        }
        if TITLE_STOPWORDS.contains(&folded.as_str()) {
            continue;
        }
        return folded;
    }
    String::new()
}

/// ASCII-fold Unicode text by applying NFKD decomposition and
/// discarding combining marks. `müller` → `muller`, `Søren` → `Soren`,
/// `ﬁnite` → `finite`. Leaves ASCII input unchanged.
pub fn ascii_fold(s: &str) -> String {
    use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
    s.nfkd().filter(|c| !is_combining_mark(*c)).collect()
}

/// Append `a`/`b`/`c`/…/`z`/`aa`/… until the candidate isn't in `taken`.
/// Empty suffix when the base is free.
#[must_use]
pub fn disambiguate<S: std::hash::BuildHasher>(base: &str, taken: &HashSet<String, S>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    for n in 0..26 * 26 {
        let suffix = letter_suffix(n);
        let candidate = format!("{base}{suffix}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    format!("{base}-overflow")
}

fn letter_suffix(n: u32) -> String {
    if n < 26 {
        std::char::from_u32(u32::from(b'a') + n)
            .unwrap_or('a')
            .to_string()
    } else {
        let first = (n / 26) - 1;
        let second = n % 26;
        format!(
            "{}{}",
            std::char::from_u32(u32::from(b'a') + first).unwrap_or('a'),
            std::char::from_u32(u32::from(b'a') + second).unwrap_or('a')
        )
    }
}

/// Assign unique keys to a batch of papers, in UUID order for the
/// collision tiebreaker. Returns one key per input paper in input
/// order. `taken` is updated with each assignment; pass it pre-
/// populated with keys already locked in the DB.
pub fn assign_keys<S: std::hash::BuildHasher>(
    papers: &[Paper],
    taken: &mut HashSet<String, S>,
) -> Vec<String> {
    let mut indexed: Vec<(usize, &Paper)> = papers.iter().enumerate().collect();
    indexed.sort_by(|a, b| a.1.id.as_str().cmp(b.1.id.as_str()));

    let mut out = vec![String::new(); papers.len()];
    for (orig_idx, paper) in indexed {
        let base = generate_key(paper);
        let assigned = disambiguate(&base, taken);
        taken.insert(assigned.clone());
        out[orig_idx] = assigned;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(title: &str, authors: &[&str], year: Option<i32>) -> Paper {
        let mut p = Paper::new(title);
        p.authors = authors.iter().map(|s| (*s).to_string()).collect();
        p.year = year;
        p
    }

    #[test]
    fn core_key_golden() {
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
            assert_eq!(generate_key(&p), *want, "title={title}");
        }
    }

    #[test]
    fn collision_suffix_by_uuid_order() {
        let mut a = paper("Theory", &["Curie, M"], Some(1903));
        let mut b = paper("Theory", &["Curie, M"], Some(1903));
        a.id = crate::models::PaperId::from("a-1111");
        b.id = crate::models::PaperId::from("b-2222");
        let mut taken = HashSet::new();
        let keys = assign_keys(&[a, b], &mut taken);
        assert_eq!(keys[0], "curie1903theory");
        assert_eq!(keys[1], "curie1903theorya");
    }
}
