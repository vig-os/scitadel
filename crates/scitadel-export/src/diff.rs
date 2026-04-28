//! Entry-level structural diff between two bibliography snapshots
//! (#135 sub-feature C).
//!
//! `bib verify` already prints a unified line-based diff when drift is
//! detected, but that output is mechanically useful and editorially
//! noisy: a one-character change in a citekey ripples through dozens of
//! lines of BibTeX, and the reader has to reconstruct the *meaning* by
//! eye. `bib diff` gives the reader the meaning directly: which entries
//! were added, which were removed, which changed (and which fields
//! changed).
//!
//! ## Identity rule
//!
//! Two entries refer to the same paper iff one of these matches —
//! tried strictly in order, first-match-wins:
//!
//! 1. citekey (exact, case-sensitive — citekeys are deterministic)
//! 2. DOI (lowercased, prefix-stripped — see [`crate::import::parse::normalize_doi`])
//! 3. arxiv_id (trimmed)
//! 4. (title, year) — last-resort fallback when all three persistent
//!    identifiers are missing, e.g. an old NeurIPS PDF with no DOI yet
//!
//! Strict per-rung means we don't try multiple at once: if citekey
//! matches we don't even *look* at DOI. This matters for ambiguous
//! cases (e.g. the citekey was renamed but DOI still matches — we
//! treat that as added/removed, not a rename, because the user's
//! manuscripts still cite by the citekey).
//!
//! ## Determinism
//!
//! All output lists are sorted by citekey lexicographically before
//! return so callers (CLI, MCP, tests) see stable output across runs
//! regardless of input order.
//!
//! ## Format-neutral
//!
//! The diff operates on [`Entry`] — a deliberately narrow shape that
//! both BibTeX and CSL-JSON parsers can populate. That way a BibTeX
//! file and a CSL-JSON file with the same papers produce zero diff
//! (verified by [`tests::mixed_format_same_papers_produce_zero_diff`]).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::import::parse::{BibEntry, normalize_doi};

/// Format-neutral bibliography entry. Populated by the BibTeX parser
/// (via [`Entry::from_bib_entry`]) or the CSL-JSON parser (via
/// [`Entry::from_csl_value`]) so the diff layer doesn't care which
/// flavor it's looking at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub citekey: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub journal: Option<String>,
    pub url: Option<String>,
    pub r#abstract: Option<String>,
}

impl Entry {
    /// Lift a parsed BibTeX entry into the format-neutral shape. DOI
    /// is already normalized by [`normalize_doi`] inside `parse_bibtex`.
    #[must_use]
    pub fn from_bib_entry(b: &BibEntry) -> Self {
        Self {
            citekey: b.citekey.clone(),
            title: b.title.clone(),
            authors: b.authors.clone(),
            year: b.year,
            doi: b.doi.clone(),
            arxiv_id: b.arxiv_id.clone(),
            journal: b
                .extra
                .get("journal")
                .or_else(|| b.extra.get("journaltitle"))
                .cloned(),
            url: b.extra.get("url").cloned(),
            r#abstract: b.extra.get("abstract").cloned(),
        }
    }

    /// Lift a single CSL-JSON object into the format-neutral shape. The
    /// canonical 1.0.2 schema fixes field names: `id`, `title`,
    /// `author[].family|given`, `issued.date-parts[0][0]`, `DOI`, `URL`,
    /// `container-title`, `abstract`. Anything else is dropped.
    pub fn from_csl_value(v: &serde_json::Value) -> Option<Self> {
        let obj = v.as_object()?;
        let citekey = obj.get("id").and_then(|x| match x {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })?;
        let title = obj
            .get("title")
            .and_then(|t| t.as_str())
            .map(str::to_string);
        let authors = obj
            .get("author")
            .and_then(|a| a.as_array())
            .map(|arr| arr.iter().filter_map(csl_author_to_string).collect())
            .unwrap_or_default();
        let year = obj
            .get("issued")
            .and_then(|d| d.get("date-parts"))
            .and_then(|dp| dp.as_array())
            .and_then(|outer| outer.first())
            .and_then(|inner| inner.as_array())
            .and_then(|inner| inner.first())
            .and_then(|n| n.as_i64())
            .map(|n| n as i32);
        let doi = obj
            .get("DOI")
            .and_then(|d| d.as_str())
            .map(normalize_doi)
            .filter(|s| !s.is_empty());
        let url = obj.get("URL").and_then(|u| u.as_str()).map(str::to_string);
        let journal = obj
            .get("container-title")
            .and_then(|c| c.as_str())
            .map(str::to_string);
        let r#abstract = obj
            .get("abstract")
            .and_then(|a| a.as_str())
            .map(str::to_string);
        // CSL doesn't carry a dedicated arxiv_id; tools that round-trip
        // arxiv ids through CSL stash them in `note` or rebuild the URL.
        // We don't try to recover that here — the diff falls through to
        // (title, year) when DOI/arxiv are both absent, which is the
        // intended pre-arxiv fallback path.
        let arxiv_id = None;
        Some(Self {
            citekey,
            title,
            authors,
            year,
            doi,
            arxiv_id,
            journal,
            url,
            r#abstract,
        })
    }
}

fn csl_author_to_string(v: &serde_json::Value) -> Option<String> {
    let obj = v.as_object()?;
    let family = obj
        .get("family")
        .and_then(|x| x.as_str())
        .or_else(|| obj.get("literal").and_then(|x| x.as_str()))?
        .trim();
    if let Some(given) = obj.get("given").and_then(|x| x.as_str())
        && !given.trim().is_empty()
    {
        Some(format!("{family}, {}", given.trim()))
    } else {
        Some(family.to_string())
    }
}

/// One field that differs between two matched entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldChange {
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// One entry whose identity matched on both sides but whose fields
/// differ. `field_changes` is sorted alphabetically by field name for
/// deterministic output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedEntry {
    pub citekey: String,
    /// Citekey on the "before" side, in case identity matched on a
    /// different rung (DOI / arxiv / title+year) and the citekey was
    /// renamed. `None` when the citekey is unchanged. Always rendered
    /// from the after-side `citekey`.
    pub before_citekey: Option<String>,
    pub field_changes: Vec<FieldChange>,
}

/// Structural diff between two entry lists. Lists are sorted by
/// citekey ascending so output is byte-stable across runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BibDiff {
    pub added: Vec<Entry>,
    pub removed: Vec<Entry>,
    pub changed: Vec<ChangedEntry>,
}

impl BibDiff {
    /// `true` iff there is no structural change. CLI maps this to
    /// exit code 0 (mirrors `git diff` semantics).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Compute the structural diff between two entry lists. Pure: no I/O,
/// no global state. Deterministic: same inputs ⇒ same output, lists
/// sorted by citekey.
#[must_use]
pub fn diff_entries(before: &[Entry], after: &[Entry]) -> BibDiff {
    // Greedy first-match-wins matching. For each `after` entry try the
    // identity rungs in order against any `before` entry not yet
    // matched. This is O(n*m) on the rungs, which is fine for the
    // shortlist sizes we expect (tens to low thousands).
    let mut matched_before: HashSet<usize> = HashSet::new();
    let mut matched_after: HashSet<usize> = HashSet::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new(); // (before_idx, after_idx)

    for (ai, a) in after.iter().enumerate() {
        if let Some(bi) = find_match(a, before, &matched_before) {
            matched_before.insert(bi);
            matched_after.insert(ai);
            pairs.push((bi, ai));
        }
    }

    let mut added: Vec<Entry> = after
        .iter()
        .enumerate()
        .filter(|(i, _)| !matched_after.contains(i))
        .map(|(_, e)| e.clone())
        .collect();
    let mut removed: Vec<Entry> = before
        .iter()
        .enumerate()
        .filter(|(i, _)| !matched_before.contains(i))
        .map(|(_, e)| e.clone())
        .collect();
    let mut changed: Vec<ChangedEntry> = pairs
        .into_iter()
        .filter_map(|(bi, ai)| {
            let b = &before[bi];
            let a = &after[ai];
            let fc = field_changes(b, a);
            if fc.is_empty() && a.citekey == b.citekey {
                None
            } else {
                let before_citekey = (a.citekey != b.citekey).then(|| b.citekey.clone());
                Some(ChangedEntry {
                    citekey: a.citekey.clone(),
                    before_citekey,
                    field_changes: fc,
                })
            }
        })
        .collect();

    added.sort_by(|a, b| a.citekey.cmp(&b.citekey));
    removed.sort_by(|a, b| a.citekey.cmp(&b.citekey));
    changed.sort_by(|a, b| a.citekey.cmp(&b.citekey));
    BibDiff {
        added,
        removed,
        changed,
    }
}

/// Try the identity rungs in strict order. Returns the index of the
/// matching `before` entry on first hit, or `None`.
fn find_match(a: &Entry, before: &[Entry], used: &HashSet<usize>) -> Option<usize> {
    // Rung 1: citekey.
    if !a.citekey.is_empty()
        && let Some(i) = before
            .iter()
            .enumerate()
            .position(|(i, b)| !used.contains(&i) && b.citekey == a.citekey)
    {
        return Some(i);
    }
    // Rung 2: DOI.
    if let Some(adoi) = a.doi.as_ref().filter(|s| !s.is_empty())
        && let Some(i) = before
            .iter()
            .enumerate()
            .position(|(i, b)| !used.contains(&i) && b.doi.as_deref() == Some(adoi.as_str()))
    {
        return Some(i);
    }
    // Rung 3: arxiv.
    if let Some(ax) = a.arxiv_id.as_ref().filter(|s| !s.is_empty())
        && let Some(i) = before
            .iter()
            .enumerate()
            .position(|(i, b)| !used.contains(&i) && b.arxiv_id.as_deref() == Some(ax.as_str()))
    {
        return Some(i);
    }
    // Rung 4: title + year.
    if let (Some(t), Some(y)) = (a.title.as_ref(), a.year)
        && !t.is_empty()
        && let Some(i) = before.iter().enumerate().position(|(i, b)| {
            !used.contains(&i) && b.title.as_deref() == Some(t.as_str()) && b.year == Some(y)
        })
    {
        return Some(i);
    }
    None
}

/// Per-field comparison. Returns a sorted-by-field list of changes.
/// Compares the format-neutral fields; authors are compared as a
/// single concatenation (`"; "`-joined) for diff output legibility.
fn field_changes(b: &Entry, a: &Entry) -> Vec<FieldChange> {
    let mut out: Vec<FieldChange> = Vec::new();
    if b.title != a.title {
        out.push(FieldChange {
            field: "title".into(),
            before: b.title.clone(),
            after: a.title.clone(),
        });
    }
    if b.authors != a.authors {
        out.push(FieldChange {
            field: "author".into(),
            before: opt_str_from_authors(&b.authors),
            after: opt_str_from_authors(&a.authors),
        });
    }
    if b.year != a.year {
        out.push(FieldChange {
            field: "year".into(),
            before: b.year.map(|y| y.to_string()),
            after: a.year.map(|y| y.to_string()),
        });
    }
    if b.doi != a.doi {
        out.push(FieldChange {
            field: "DOI".into(),
            before: b.doi.clone(),
            after: a.doi.clone(),
        });
    }
    if b.arxiv_id != a.arxiv_id {
        out.push(FieldChange {
            field: "arxiv_id".into(),
            before: b.arxiv_id.clone(),
            after: a.arxiv_id.clone(),
        });
    }
    if b.journal != a.journal {
        out.push(FieldChange {
            field: "journal".into(),
            before: b.journal.clone(),
            after: a.journal.clone(),
        });
    }
    if b.url != a.url {
        out.push(FieldChange {
            field: "URL".into(),
            before: b.url.clone(),
            after: a.url.clone(),
        });
    }
    if b.r#abstract != a.r#abstract {
        out.push(FieldChange {
            field: "abstract".into(),
            before: b.r#abstract.clone(),
            after: a.r#abstract.clone(),
        });
    }
    out.sort_by(|x, y| x.field.cmp(&y.field));
    out
}

fn opt_str_from_authors(authors: &[String]) -> Option<String> {
    if authors.is_empty() {
        None
    } else {
        Some(authors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(citekey: &str) -> Entry {
        Entry {
            citekey: citekey.into(),
            ..Default::default()
        }
    }

    fn full(citekey: &str, title: &str, year: i32) -> Entry {
        Entry {
            citekey: citekey.into(),
            title: Some(title.into()),
            year: Some(year),
            authors: vec!["Smith, J.".into()],
            ..Default::default()
        }
    }

    // ---------- Identity rule: 4 separate tests, one per rung ----------

    #[test]
    fn identity_rung1_citekey_match_wins() {
        let a = full("smith2024", "T", 2024);
        let b = full("smith2024", "T", 2024);
        let d = diff_entries(&[a], &[b]);
        assert!(d.is_empty(), "same citekey + identical fields ⇒ no diff");
    }

    #[test]
    fn identity_rung1_citekey_takes_priority_over_doi() {
        // before has citekey=X but DOI=10.1/abc
        let mut before = full("X", "Title", 2024);
        before.doi = Some("10.1/abc".into());
        // after has different citekey but same DOI — by the rule, the
        // citekey doesn't match, so we fall through to DOI rung 2.
        // BUT: we want to verify rung 1 strictness. So set up TWO
        // before entries: one with the citekey, one with the DOI. The
        // citekey match must win.
        let mut before_decoy = full("Y", "DecoyTitle", 2020);
        before_decoy.doi = Some("10.1/abc".into());
        let mut after = full("X", "Title", 2024);
        after.doi = Some("10.1/abc".into());

        let d = diff_entries(&[before_decoy.clone(), before], &[after]);
        // The citekey-matched entry should be matched (no diff for it),
        // and the decoy should appear in `removed` (it's still in
        // before but not after).
        assert_eq!(d.removed.len(), 1, "decoy must be removed: {d:?}");
        assert_eq!(d.removed[0].citekey, "Y");
        assert!(d.changed.is_empty());
        assert!(d.added.is_empty());
    }

    #[test]
    fn identity_rung2_doi_when_citekeys_differ() {
        let mut before = full("oldkey", "Title", 2024);
        before.doi = Some("10.1/abc".into());
        let mut after = full("newkey", "Title", 2024);
        after.doi = Some("10.1/abc".into());
        let d = diff_entries(&[before], &[after]);
        // DOI matched ⇒ same paper, but citekey changed.
        assert_eq!(d.changed.len(), 1, "DOI rung must match: {d:?}");
        assert_eq!(d.changed[0].citekey, "newkey");
        assert_eq!(d.changed[0].before_citekey.as_deref(), Some("oldkey"));
        assert!(d.added.is_empty());
        assert!(d.removed.is_empty());
    }

    #[test]
    fn identity_rung3_arxiv_when_doi_absent() {
        let mut before = full("oldkey", "Title", 2024);
        before.arxiv_id = Some("2301.00001".into());
        let mut after = full("newkey", "Title", 2024);
        after.arxiv_id = Some("2301.00001".into());
        let d = diff_entries(&[before], &[after]);
        assert_eq!(d.changed.len(), 1, "arxiv rung must match: {d:?}");
        assert_eq!(d.changed[0].citekey, "newkey");
    }

    #[test]
    fn identity_rung4_title_year_when_all_else_absent() {
        let before = full("oldkey", "A Common Title", 2024);
        let after = full("newkey", "A Common Title", 2024);
        let d = diff_entries(&[before], &[after]);
        assert_eq!(d.changed.len(), 1, "title+year rung must match: {d:?}");
        assert_eq!(d.changed[0].citekey, "newkey");
    }

    #[test]
    fn identity_no_match_returns_added_and_removed() {
        let before = full("a", "Alpha", 2024);
        let after = full("b", "Beta", 2025);
        let d = diff_entries(&[before], &[after]);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].citekey, "b");
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].citekey, "a");
        assert!(d.changed.is_empty());
    }

    // ---------- Field-change detection: one field at a time ----------

    fn changed_only(b: Entry, a: Entry) -> Vec<FieldChange> {
        let d = diff_entries(&[b], &[a]);
        assert_eq!(d.changed.len(), 1);
        d.changed.into_iter().next().unwrap().field_changes
    }

    #[test]
    fn field_change_title() {
        let mut b = full("k", "Old", 2024);
        let mut a = full("k", "New", 2024);
        b.authors = vec!["X".into()];
        a.authors = vec!["X".into()];
        let fc = changed_only(b, a);
        assert_eq!(fc.len(), 1, "only title changed: {fc:?}");
        assert_eq!(fc[0].field, "title");
        assert_eq!(fc[0].before.as_deref(), Some("Old"));
        assert_eq!(fc[0].after.as_deref(), Some("New"));
    }

    #[test]
    fn field_change_year_only() {
        let mut b = full("k", "T", 2023);
        let mut a = full("k", "T", 2024);
        b.authors = vec!["X".into()];
        a.authors = vec!["X".into()];
        let fc = changed_only(b, a);
        assert_eq!(fc.len(), 1, "only year changed: {fc:?}");
        assert_eq!(fc[0].field, "year");
    }

    #[test]
    fn field_change_author_only() {
        let mut b = full("k", "T", 2024);
        let mut a = full("k", "T", 2024);
        b.authors = vec!["Smith, J.".into()];
        a.authors = vec!["Smith, J.".into(), "Doe, J.".into()];
        let fc = changed_only(b, a);
        assert_eq!(fc.len(), 1, "only author changed: {fc:?}");
        assert_eq!(fc[0].field, "author");
    }

    #[test]
    fn field_change_doi_only() {
        let mut b = full("k", "T", 2024);
        let mut a = full("k", "T", 2024);
        b.doi = Some("10.1/old".into());
        a.doi = Some("10.1/new".into());
        let fc = changed_only(b, a);
        assert_eq!(fc.len(), 1, "only DOI changed: {fc:?}");
        assert_eq!(fc[0].field, "DOI");
    }

    #[test]
    fn field_change_journal_only() {
        let mut b = full("k", "T", 2024);
        let mut a = full("k", "T", 2024);
        b.journal = Some("Nature".into());
        a.journal = Some("Science".into());
        let fc = changed_only(b, a);
        assert_eq!(fc.len(), 1, "only journal changed: {fc:?}");
        assert_eq!(fc[0].field, "journal");
    }

    #[test]
    fn field_change_abstract_only() {
        let mut b = full("k", "T", 2024);
        let mut a = full("k", "T", 2024);
        b.r#abstract = Some("old abs".into());
        a.r#abstract = Some("new abs".into());
        let fc = changed_only(b, a);
        assert_eq!(fc.len(), 1, "only abstract changed: {fc:?}");
        assert_eq!(fc[0].field, "abstract");
    }

    // ---------- Determinism ----------

    #[test]
    fn sort_determinism_added_removed_changed() {
        // Different insertion orders, same content ⇒ same output.
        let b1 = full("alpha", "A", 2024);
        let b2 = full("zeta", "Z", 2024);
        let b3 = full("mu", "M", 2024);
        let a1 = full("beta", "B", 2024);
        let a2 = full("zeta", "Z2", 2024); // changed title
        let a3 = full("nu", "N", 2024);

        let r1 = diff_entries(
            &[b1.clone(), b2.clone(), b3.clone()],
            &[a1.clone(), a2.clone(), a3.clone()],
        );
        let r2 = diff_entries(&[b3, b1, b2], &[a3, a2, a1]);

        let added_keys = |d: &BibDiff| {
            d.added
                .iter()
                .map(|e| e.citekey.clone())
                .collect::<Vec<_>>()
        };
        let removed_keys = |d: &BibDiff| {
            d.removed
                .iter()
                .map(|e| e.citekey.clone())
                .collect::<Vec<_>>()
        };
        let changed_keys = |d: &BibDiff| {
            d.changed
                .iter()
                .map(|c| c.citekey.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(added_keys(&r1), added_keys(&r2));
        assert_eq!(removed_keys(&r1), removed_keys(&r2));
        assert_eq!(changed_keys(&r1), changed_keys(&r2));
        // And actually sorted:
        assert_eq!(added_keys(&r1), vec!["beta", "nu"]);
        assert_eq!(removed_keys(&r1), vec!["alpha", "mu"]);
        assert_eq!(changed_keys(&r1), vec!["zeta"]);
    }

    // ---------- JSON round-trip ----------

    #[test]
    fn json_round_trip_matches_struct() {
        let b = full("a", "Old", 2023);
        let a = full("a", "New", 2024);
        let extra = full("b", "Beta", 2024);
        let d = diff_entries(&[b], &[a, extra]);
        let json = serde_json::to_string(&d).unwrap();
        let back: BibDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    // ---------- Mixed-format diff (BibTeX vs CSL-JSON) ----------

    #[test]
    fn mixed_format_same_papers_produce_zero_diff() {
        // Build a BibTeX doc and a CSL-JSON doc that describe the same
        // two papers; lift to Entry via both paths; assert empty diff.
        let bib_src = r"
@article{smith2024quantum,
    title = {Quantum Advantage},
    author = {Smith, John and Doe, Jane},
    year = {2024},
    doi = {10.1038/nature.2024}
}
@article{lee2023neural,
    title = {Neural Y},
    author = {Lee, Kai},
    year = {2023}
}
        ";
        let bib_entries = crate::import::parse::parse_bibtex(bib_src).unwrap();
        let bib_neutral: Vec<Entry> = bib_entries.iter().map(Entry::from_bib_entry).collect();

        let csl_src = serde_json::json!([
            {
                "id": "smith2024quantum",
                "type": "article-journal",
                "title": "Quantum Advantage",
                "author": [
                    {"family": "Smith", "given": "John"},
                    {"family": "Doe", "given": "Jane"}
                ],
                "issued": {"date-parts": [[2024]]},
                "DOI": "10.1038/nature.2024"
            },
            {
                "id": "lee2023neural",
                "type": "article-journal",
                "title": "Neural Y",
                "author": [{"family": "Lee", "given": "Kai"}],
                "issued": {"date-parts": [[2023]]}
            }
        ]);
        let csl_arr = csl_src.as_array().unwrap();
        let csl_neutral: Vec<Entry> = csl_arr.iter().filter_map(Entry::from_csl_value).collect();
        assert_eq!(csl_neutral.len(), 2);

        let d = diff_entries(&bib_neutral, &csl_neutral);
        assert!(
            d.is_empty(),
            "BibTeX-vs-CSL of the same papers must produce no diff: {d:?}"
        );
    }

    // ---------- Identity strictness ----------

    #[test]
    fn identity_strict_per_rung_does_not_combine() {
        // before: citekey=X, no DOI
        // after:  citekey=Y, DOI=10/abc, no overlap
        // ⇒ no match (citekey miss; DOI not present in `before`)
        let before = entry("X");
        let mut after = entry("Y");
        after.doi = Some("10/abc".into());
        let d = diff_entries(&[before], &[after]);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.removed.len(), 1);
    }

    #[test]
    fn empty_inputs_are_a_no_op() {
        let d = diff_entries(&[], &[]);
        assert!(d.is_empty());
    }
}
