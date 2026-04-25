//! Merge strategies for `.bib` import (#134 step 3).
//!
//! Resolves what happens when a [`BibEntry`] matches an existing
//! [`Paper`] in the DB:
//!
//! - [`MergeStrategy::Reject`] — never write. Surface a "skipped"
//!   line in the import summary.
//! - [`MergeStrategy::DbWins`] — DB stays exactly as is. Aliases and
//!   step-4 side effects (annotations, tags) are still recorded by
//!   the caller — strategy controls only the `Paper` row.
//! - [`MergeStrategy::BibWins`] — overwrite DB with bib values where
//!   bib has them. Bib `None` does NOT wipe DB values.
//! - [`MergeStrategy::Merge`] (default) — DB wins on scitadel-owned
//!   fields (title/authors/year/journal/doi/arxiv_id/pubmed_id/
//!   openalex_id/url/abstract). Non-owned fields (`note=`,
//!   `keywords=`, `file=`) are left to step-4 side-effect plumbing.
//!   The Paper row itself stays unchanged under Merge — the value
//!   the strategy adds is in the side effects, not the columns.
//!
//! Note: `interactive` mode is P1 polish per the issue — not in iter 2.

use scitadel_core::models::Paper;

use super::parse::BibEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    Reject,
    DbWins,
    BibWins,
    Merge,
}

impl MergeStrategy {
    /// Parse the `--strategy` CLI flag value. Unknown strings → `None`
    /// so the caller can produce a `clap`-style error with the legal
    /// values listed.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reject" => Some(Self::Reject),
            "db-wins" => Some(Self::DbWins),
            "bib-wins" => Some(Self::BibWins),
            "merge" => Some(Self::Merge),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::DbWins => "db-wins",
            Self::BibWins => "bib-wins",
            Self::Merge => "merge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeAction {
    /// No DB match — a fresh paper was created from the bib entry.
    Created,
    /// Existing paper modified by the strategy.
    Updated,
    /// Match found, strategy left the paper untouched.
    Unchanged,
    /// Match found, strategy = Reject — caller skips the write.
    Rejected,
}

#[derive(Debug, Clone)]
pub struct MergeOutcome {
    /// Resolved paper. `None` only for `Rejected`.
    pub paper: Option<Paper>,
    pub action: MergeAction,
    /// Field names taken from the bib (used by the import summary
    /// line, e.g. `"merged p-abc (2 fields from bib: note, keywords)"`).
    pub from_bib: Vec<&'static str>,
    /// DB-side fields preserved against a bib value (used by the same
    /// summary line: `"... kept DB title"`).
    pub kept_from_db: Vec<&'static str>,
}

/// Resolve the outcome of importing one bib entry. `db_paper = None`
/// means no match — a fresh paper is created regardless of strategy.
pub fn resolve(
    db_paper: Option<Paper>,
    bib: &BibEntry,
    strategy: MergeStrategy,
) -> MergeOutcome {
    let Some(db) = db_paper else {
        return MergeOutcome {
            paper: Some(paper_from_bib(bib)),
            action: MergeAction::Created,
            from_bib: bib_present_owned_fields(bib),
            kept_from_db: vec![],
        };
    };

    match strategy {
        MergeStrategy::Reject => MergeOutcome {
            paper: None,
            action: MergeAction::Rejected,
            from_bib: vec![],
            kept_from_db: vec![],
        },
        // DbWins and Merge leave the Paper row identical — Merge
        // adds value via side effects (annotations from `note=`,
        // tags from `keywords=`) wired in step 4, not by mutating
        // owned columns. Collapsed to one arm to keep clippy happy.
        MergeStrategy::DbWins | MergeStrategy::Merge => MergeOutcome {
            paper: Some(db),
            action: MergeAction::Unchanged,
            from_bib: vec![],
            kept_from_db: bib_present_owned_fields(bib),
        },
        MergeStrategy::BibWins => bib_wins(db, bib),
    }
}

/// Build a fresh `Paper` from a [`BibEntry`]. Used for unmatched
/// imports (no DB row exists yet). `bibtex_key` is left `None` so the
/// DB save path assigns one via the #132 algorithm rather than
/// blindly trusting the imported citekey, which would skip
/// disambiguation against existing keys.
pub fn paper_from_bib(bib: &BibEntry) -> Paper {
    let title = bib.title.clone().unwrap_or_default();
    let mut p = Paper::new(title);
    p.authors.clone_from(&bib.authors);
    p.year = bib.year;
    p.doi.clone_from(&bib.doi);
    p.arxiv_id.clone_from(&bib.arxiv_id);
    p.pubmed_id.clone_from(&bib.pubmed_id);
    p.openalex_id.clone_from(&bib.openalex_id);
    if let Some(j) = bib.extra.get("journal").or_else(|| bib.extra.get("journaltitle")) {
        p.journal = Some(j.clone());
    }
    if let Some(u) = bib.extra.get("url") {
        p.url = Some(u.clone());
    }
    if let Some(a) = bib.extra.get("abstract") {
        p.r#abstract.clone_from(a);
    }
    p
}

/// Apply `BibWins` semantics: bib values overwrite DB values where
/// bib has Some, but bib `None` never erases DB content.
fn bib_wins(db: Paper, bib: &BibEntry) -> MergeOutcome {
    let mut out = db.clone();
    let mut from_bib: Vec<&'static str> = Vec::new();
    let mut kept_from_db: Vec<&'static str> = Vec::new();

    macro_rules! override_opt {
        ($field:ident, $name:literal) => {
            match (&bib.$field, &out.$field) {
                (Some(v), prev) if prev.as_ref() != Some(v) => {
                    out.$field = Some(v.clone());
                    from_bib.push($name);
                }
                (Some(_), _) => {}
                (None, Some(_)) => kept_from_db.push($name),
                (None, None) => {}
            }
        };
    }

    if let Some(t) = bib.title.as_ref()
        && &out.title != t
    {
        out.title.clone_from(t);
        from_bib.push("title");
    }
    if !bib.authors.is_empty() && bib.authors != out.authors {
        out.authors.clone_from(&bib.authors);
        from_bib.push("authors");
    } else if bib.authors.is_empty() && !out.authors.is_empty() {
        kept_from_db.push("authors");
    }
    match (bib.year, out.year) {
        (Some(y), prev) if prev != Some(y) => {
            out.year = Some(y);
            from_bib.push("year");
        }
        (None, Some(_)) => kept_from_db.push("year"),
        _ => {}
    }
    override_opt!(doi, "doi");
    override_opt!(arxiv_id, "arxiv_id");
    override_opt!(pubmed_id, "pubmed_id");
    override_opt!(openalex_id, "openalex_id");

    // Extra fields: journal / url / abstract are scitadel-owned but
    // come through `extra` because biblatex doesn't expose dedicated
    // accessors for all of them. Override only when bib has them.
    if let Some(j) = bib.extra.get("journal").or_else(|| bib.extra.get("journaltitle")) {
        if out.journal.as_deref() != Some(j.as_str()) {
            out.journal = Some(j.clone());
            from_bib.push("journal");
        }
    } else if out.journal.is_some() {
        kept_from_db.push("journal");
    }
    if let Some(u) = bib.extra.get("url") {
        if out.url.as_deref() != Some(u.as_str()) {
            out.url = Some(u.clone());
            from_bib.push("url");
        }
    } else if out.url.is_some() {
        kept_from_db.push("url");
    }

    let action = if from_bib.is_empty() {
        MergeAction::Unchanged
    } else {
        MergeAction::Updated
    };
    MergeOutcome {
        paper: Some(out),
        action,
        from_bib,
        kept_from_db,
    }
}

/// Names of scitadel-owned fields the bib carries values for. Drives
/// the audit summary "kept DB X" lines under Merge / DbWins strategies.
fn bib_present_owned_fields(bib: &BibEntry) -> Vec<&'static str> {
    let mut v = Vec::new();
    if bib.title.is_some() {
        v.push("title");
    }
    if !bib.authors.is_empty() {
        v.push("authors");
    }
    if bib.year.is_some() {
        v.push("year");
    }
    if bib.doi.is_some() {
        v.push("doi");
    }
    if bib.arxiv_id.is_some() {
        v.push("arxiv_id");
    }
    if bib.pubmed_id.is_some() {
        v.push("pubmed_id");
    }
    if bib.openalex_id.is_some() {
        v.push("openalex_id");
    }
    if bib.extra.contains_key("journal") || bib.extra.contains_key("journaltitle") {
        v.push("journal");
    }
    if bib.extra.contains_key("url") {
        v.push("url");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn full_bib() -> BibEntry {
        let mut extra = HashMap::new();
        extra.insert("journal".into(), "Nature".into());
        extra.insert("url".into(), "https://example.com/paper".into());
        BibEntry {
            citekey: "k".into(),
            title: Some("Bib Title".into()),
            authors: vec!["Smith, John".into()],
            year: Some(2024),
            doi: Some("10.1/bib".into()),
            arxiv_id: Some("2401.0001".into()),
            pubmed_id: Some("12345".into()),
            openalex_id: Some("W1".into()),
            note: None,
            keywords: vec![],
            file: None,
            extra,
        }
    }

    fn db_paper() -> Paper {
        let mut p = Paper::new("DB Title");
        p.authors = vec!["DB Author".into()];
        p.year = Some(2020);
        p.doi = Some("10.1/db".into());
        p.journal = Some("DB Journal".into());
        p.bibtex_key = Some("dbkey".into());
        p
    }

    #[test]
    fn parse_strategy_round_trip() {
        for s in ["reject", "db-wins", "bib-wins", "merge"] {
            let parsed = MergeStrategy::parse(s).unwrap();
            assert_eq!(parsed.as_str(), s);
        }
        assert_eq!(MergeStrategy::parse("nope"), None);
    }

    #[test]
    fn no_db_match_creates_fresh_paper() {
        let bib = full_bib();
        let out = resolve(None, &bib, MergeStrategy::Merge);
        assert_eq!(out.action, MergeAction::Created);
        let p = out.paper.unwrap();
        assert_eq!(p.title, "Bib Title");
        assert_eq!(p.authors, vec!["Smith, John"]);
        assert_eq!(p.year, Some(2024));
        assert_eq!(p.doi.as_deref(), Some("10.1/bib"));
        assert_eq!(p.journal.as_deref(), Some("Nature"));
        assert_eq!(p.url.as_deref(), Some("https://example.com/paper"));
        // bibtex_key intentionally None — the DB layer assigns via
        // the #132 algorithm so the imported citekey doesn't bypass
        // collision disambiguation.
        assert_eq!(p.bibtex_key, None);
    }

    #[test]
    fn reject_strategy_with_match_yields_no_paper() {
        let out = resolve(Some(db_paper()), &full_bib(), MergeStrategy::Reject);
        assert_eq!(out.action, MergeAction::Rejected);
        assert!(out.paper.is_none());
    }

    #[test]
    fn db_wins_with_match_keeps_db_unchanged() {
        let bib = full_bib();
        let out = resolve(Some(db_paper()), &bib, MergeStrategy::DbWins);
        assert_eq!(out.action, MergeAction::Unchanged);
        let p = out.paper.unwrap();
        assert_eq!(p.title, "DB Title");
        assert_eq!(p.year, Some(2020));
        assert_eq!(p.doi.as_deref(), Some("10.1/db"));
        // kept_from_db lists every owned field bib was carrying — the
        // CLI uses this to print "kept DB title, kept DB authors, …".
        assert!(out.kept_from_db.contains(&"title"));
        assert!(out.kept_from_db.contains(&"authors"));
        assert!(out.kept_from_db.contains(&"doi"));
    }

    #[test]
    fn merge_strategy_treats_db_owned_fields_as_authoritative() {
        let bib = full_bib();
        let out = resolve(Some(db_paper()), &bib, MergeStrategy::Merge);
        // Merge leaves the Paper row alone; side effects (annotations
        // from note, tags from keywords) come from step-4 plumbing.
        assert_eq!(out.action, MergeAction::Unchanged);
        let p = out.paper.unwrap();
        assert_eq!(p.title, "DB Title");
        assert_eq!(p.doi.as_deref(), Some("10.1/db"));
        assert!(out.kept_from_db.contains(&"title"));
    }

    #[test]
    fn bib_wins_overrides_with_bib_values_only_when_present() {
        let bib = full_bib();
        let out = resolve(Some(db_paper()), &bib, MergeStrategy::BibWins);
        assert_eq!(out.action, MergeAction::Updated);
        let p = out.paper.unwrap();
        assert_eq!(p.title, "Bib Title");
        assert_eq!(p.authors, vec!["Smith, John"]);
        assert_eq!(p.year, Some(2024));
        assert_eq!(p.doi.as_deref(), Some("10.1/bib"));
        assert_eq!(p.arxiv_id.as_deref(), Some("2401.0001"));
        assert_eq!(p.journal.as_deref(), Some("Nature"));
        assert!(out.from_bib.contains(&"title"));
        assert!(out.from_bib.contains(&"authors"));
        assert!(out.from_bib.contains(&"doi"));
    }

    #[test]
    fn bib_wins_does_not_wipe_db_when_bib_field_missing() {
        let mut bib = full_bib();
        bib.title = None;
        bib.year = None;
        bib.doi = None;
        let out = resolve(Some(db_paper()), &bib, MergeStrategy::BibWins);
        let p = out.paper.unwrap();
        assert_eq!(p.title, "DB Title", "absent bib title must not blank DB title");
        assert_eq!(p.year, Some(2020));
        assert_eq!(p.doi.as_deref(), Some("10.1/db"));
        assert!(out.kept_from_db.contains(&"year"));
        assert!(out.kept_from_db.contains(&"doi"));
    }

    #[test]
    fn bib_wins_with_identical_values_is_unchanged() {
        let mut bib = full_bib();
        bib.title = Some("DB Title".into());
        bib.authors = vec!["DB Author".into()];
        bib.year = Some(2020);
        bib.doi = Some("10.1/db".into());
        bib.arxiv_id = None;
        bib.pubmed_id = None;
        bib.openalex_id = None;
        bib.extra.clear();
        bib.extra
            .insert("journal".into(), "DB Journal".into());

        let out = resolve(Some(db_paper()), &bib, MergeStrategy::BibWins);
        assert_eq!(out.action, MergeAction::Unchanged, "identical bib = no diff");
        assert!(out.from_bib.is_empty());
    }

    #[test]
    fn round_trip_invariant_paper_from_bib_then_bib_wins_idempotent() {
        // Fresh paper from a bib entry, then re-resolve with the same
        // bib via BibWins must report Unchanged. This is a unit-level
        // proxy for the #134 round-trip pitfall: scitadel export →
        // import touches zero rows.
        let bib = full_bib();
        let p = paper_from_bib(&bib);
        let out = resolve(Some(p), &bib, MergeStrategy::BibWins);
        assert_eq!(
            out.action,
            MergeAction::Unchanged,
            "paper_from_bib + bib-wins must be idempotent; from_bib={:?}",
            out.from_bib,
        );
    }
}
