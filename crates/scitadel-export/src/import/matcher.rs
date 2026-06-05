//! Match a [`BibEntry`] to an existing paper via the cascade pinned
//! in issue #134:
//!
//! 1. DOI (normalized)
//! 2. arXiv id
//! 3. PubMed id
//! 4. OpenAlex id
//! 5. **scitadel's own `bibtex_key`** — covers the round-trip case
//!    where a `.bib` was previously exported by scitadel and is being
//!    re-imported without ever leaving a DOI trail.
//! 6. **Alias from `paper_aliases`** (#134 step 1) — covers the
//!    re-import of a third-party `.bib` where the citekey was
//!    preserved during the previous import. Ambiguous aliases (the
//!    alias points at >1 paper) are skipped so the cascade falls
//!    through to title+year instead of silently picking.
//! 7. Title + year exact match (case-insensitive, normalized whitespace).
//!    No fuzzy matching — that's out of scope for iter 2 and prone to
//!    false positives.
//!
//! The matcher is isolated behind the [`PaperLookup`] trait so the
//! cascade can be unit-tested without a SQLite fixture. The concrete
//! impl backed by `SqlitePaperRepository` + `SqlitePaperAliasRepository`
//! lives at the CLI/MCP wiring layer (step 5).

use super::parse::BibEntry;
use scitadel_core::error::CoreError;

/// Narrow port the matcher depends on. Each method returns the
/// matching paper's `id` (not the full `Paper`) so the cascade is
/// allocation-light — merging + field diff happens upstream once the
/// match is resolved.
pub trait PaperLookup {
    fn find_by_doi(&self, doi: &str) -> Result<Option<String>, CoreError>;
    fn find_by_arxiv_id(&self, id: &str) -> Result<Option<String>, CoreError>;
    fn find_by_pubmed_id(&self, id: &str) -> Result<Option<String>, CoreError>;
    fn find_by_openalex_id(&self, id: &str) -> Result<Option<String>, CoreError>;
    /// Scitadel's authoritative citation key (column `papers.bibtex_key`).
    fn find_by_bibtex_key(&self, key: &str) -> Result<Option<String>, CoreError>;
    /// All papers sharing this alias, for ambiguity detection. See
    /// `SqlitePaperAliasRepository::lookup_all`.
    fn find_by_alias(&self, alias: &str) -> Result<Vec<String>, CoreError>;
    /// Case-insensitive exact title match, optionally constrained by year.
    fn find_by_title_and_year(
        &self,
        title: &str,
        year: Option<i32>,
    ) -> Result<Option<String>, CoreError>;
}

/// Outcome of a single match attempt. Explicit `Ambiguous` variant so
/// step 3's merge strategy layer can surface "you have two papers
/// sharing an alias — pick one" interactively instead of silently
/// picking whichever SQL returned first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    /// `(paper_id, strategy_that_matched)` — the strategy is surfaced
    /// in the per-paper import summary line ("matched via DOI", "via
    /// title+year", etc).
    Matched(String, MatchStrategy),
    /// No strategy produced a hit. New paper — import creates one.
    NoMatch,
    /// Alias resolved to multiple papers. Step 3 may prompt the user;
    /// default is to fall through to title+year (which this variant
    /// pre-computes so the caller doesn't re-run the cascade).
    AmbiguousAlias(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchStrategy {
    Doi,
    ArxivId,
    PubmedId,
    OpenalexId,
    BibtexKey,
    Alias,
    TitleAndYear,
}

impl MatchStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Doi => "doi",
            Self::ArxivId => "arxiv_id",
            Self::PubmedId => "pubmed_id",
            Self::OpenalexId => "openalex_id",
            Self::BibtexKey => "bibtex_key",
            Self::Alias => "alias",
            Self::TitleAndYear => "title+year",
        }
    }
}

/// Run the full cascade on one entry. Short-circuits on the first hit.
pub fn match_entry(entry: &BibEntry, lookup: &dyn PaperLookup) -> Result<MatchOutcome, CoreError> {
    if let Some(doi) = entry.doi.as_deref()
        && let Some(id) = lookup.find_by_doi(doi)?
    {
        return Ok(MatchOutcome::Matched(id, MatchStrategy::Doi));
    }
    if let Some(arxiv) = entry.arxiv_id.as_deref()
        && let Some(id) = lookup.find_by_arxiv_id(arxiv)?
    {
        return Ok(MatchOutcome::Matched(id, MatchStrategy::ArxivId));
    }
    if let Some(pmid) = entry.pubmed_id.as_deref()
        && let Some(id) = lookup.find_by_pubmed_id(pmid)?
    {
        return Ok(MatchOutcome::Matched(id, MatchStrategy::PubmedId));
    }
    if let Some(oa) = entry.openalex_id.as_deref()
        && let Some(id) = lookup.find_by_openalex_id(oa)?
    {
        return Ok(MatchOutcome::Matched(id, MatchStrategy::OpenalexId));
    }
    // Scitadel-own key: catches export→import round-trip cleanly.
    if let Some(id) = lookup.find_by_bibtex_key(&entry.citekey)? {
        return Ok(MatchOutcome::Matched(id, MatchStrategy::BibtexKey));
    }
    // Alias: catches re-import of a third-party .bib. Ambiguous
    // aliases fall through; unambiguous hits resolve.
    let alias_hits = lookup.find_by_alias(&entry.citekey)?;
    match alias_hits.len() {
        0 => {}
        1 => {
            return Ok(MatchOutcome::Matched(
                alias_hits[0].clone(),
                MatchStrategy::Alias,
            ));
        }
        _ => {
            // Record the ambiguity so the caller can surface it, then
            // fall through so the per-paper strategy still runs
            // title+year and potentially converts the ambiguous alias
            // into a clean title match.
            if let Some(title) = entry.title.as_deref()
                && let Some(id) = lookup.find_by_title_and_year(title, entry.year)?
            {
                return Ok(MatchOutcome::Matched(id, MatchStrategy::TitleAndYear));
            }
            return Ok(MatchOutcome::AmbiguousAlias(alias_hits));
        }
    }
    if let Some(title) = entry.title.as_deref()
        && let Some(id) = lookup.find_by_title_and_year(title, entry.year)?
    {
        return Ok(MatchOutcome::Matched(id, MatchStrategy::TitleAndYear));
    }
    Ok(MatchOutcome::NoMatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MockLookup {
        by_doi: HashMap<String, String>,
        by_arxiv: HashMap<String, String>,
        by_pubmed: HashMap<String, String>,
        by_openalex: HashMap<String, String>,
        by_bibtex_key: HashMap<String, String>,
        by_alias: HashMap<String, Vec<String>>,
        by_title: HashMap<(String, Option<i32>), String>,
        /// Recorded call order so tests can verify the cascade
        /// short-circuits on the first hit.
        call_log: RefCell<Vec<&'static str>>,
    }

    impl MockLookup {
        fn log(&self, step: &'static str) {
            self.call_log.borrow_mut().push(step);
        }
    }

    impl PaperLookup for MockLookup {
        fn find_by_doi(&self, doi: &str) -> Result<Option<String>, CoreError> {
            self.log("doi");
            Ok(self.by_doi.get(doi).cloned())
        }
        fn find_by_arxiv_id(&self, id: &str) -> Result<Option<String>, CoreError> {
            self.log("arxiv");
            Ok(self.by_arxiv.get(id).cloned())
        }
        fn find_by_pubmed_id(&self, id: &str) -> Result<Option<String>, CoreError> {
            self.log("pubmed");
            Ok(self.by_pubmed.get(id).cloned())
        }
        fn find_by_openalex_id(&self, id: &str) -> Result<Option<String>, CoreError> {
            self.log("openalex");
            Ok(self.by_openalex.get(id).cloned())
        }
        fn find_by_bibtex_key(&self, key: &str) -> Result<Option<String>, CoreError> {
            self.log("bibtex_key");
            Ok(self.by_bibtex_key.get(key).cloned())
        }
        fn find_by_alias(&self, alias: &str) -> Result<Vec<String>, CoreError> {
            self.log("alias");
            Ok(self.by_alias.get(alias).cloned().unwrap_or_default())
        }
        fn find_by_title_and_year(
            &self,
            title: &str,
            year: Option<i32>,
        ) -> Result<Option<String>, CoreError> {
            self.log("title_year");
            Ok(self.by_title.get(&(title.to_lowercase(), year)).cloned())
        }
    }

    fn bare_entry(citekey: &str) -> BibEntry {
        BibEntry {
            citekey: citekey.into(),
            title: None,
            authors: vec![],
            year: None,
            doi: None,
            arxiv_id: None,
            pubmed_id: None,
            openalex_id: None,
            note: None,
            keywords: vec![],
            file: None,
            extra: HashMap::default(),
        }
    }

    #[test]
    fn doi_match_short_circuits_before_arxiv() {
        let mut lookup = MockLookup::default();
        lookup.by_doi.insert("10.1/x".into(), "p-1".into());
        // arxiv would also match, but DOI wins the cascade.
        lookup.by_arxiv.insert("2301.1".into(), "p-2".into());

        let mut e = bare_entry("k");
        e.doi = Some("10.1/x".into());
        e.arxiv_id = Some("2301.1".into());

        let out = match_entry(&e, &lookup).unwrap();
        assert_eq!(out, MatchOutcome::Matched("p-1".into(), MatchStrategy::Doi));
        assert_eq!(*lookup.call_log.borrow(), vec!["doi"]);
    }

    #[test]
    fn cascade_falls_through_doi_arxiv_pubmed_openalex_in_order() {
        let mut lookup = MockLookup::default();
        lookup.by_openalex.insert("W1".into(), "p-oa".into());

        let mut e = bare_entry("k");
        e.doi = Some("nope".into());
        e.arxiv_id = Some("nope".into());
        e.pubmed_id = Some("nope".into());
        e.openalex_id = Some("W1".into());

        let out = match_entry(&e, &lookup).unwrap();
        assert_eq!(
            out,
            MatchOutcome::Matched("p-oa".into(), MatchStrategy::OpenalexId)
        );
        assert_eq!(
            *lookup.call_log.borrow(),
            vec!["doi", "arxiv", "pubmed", "openalex"]
        );
    }

    #[test]
    fn bibtex_key_catches_round_trip_without_external_ids() {
        let mut lookup = MockLookup::default();
        lookup
            .by_bibtex_key
            .insert("smith2024quantum".into(), "p-rt".into());

        let e = bare_entry("smith2024quantum");
        let out = match_entry(&e, &lookup).unwrap();
        assert_eq!(
            out,
            MatchOutcome::Matched("p-rt".into(), MatchStrategy::BibtexKey)
        );
    }

    #[test]
    fn alias_resolves_when_single_hit() {
        let mut lookup = MockLookup::default();
        lookup
            .by_alias
            .insert("zotero_old_key".into(), vec!["p-aliased".into()]);

        let e = bare_entry("zotero_old_key");
        let out = match_entry(&e, &lookup).unwrap();
        assert_eq!(
            out,
            MatchOutcome::Matched("p-aliased".into(), MatchStrategy::Alias)
        );
    }

    #[test]
    fn ambiguous_alias_falls_through_to_title_year_when_possible() {
        let mut lookup = MockLookup::default();
        lookup
            .by_alias
            .insert("smith2024".into(), vec!["p-a".into(), "p-b".into()]);
        lookup
            .by_title
            .insert(("quantum theory".into(), Some(2024)), "p-c".into());

        let mut e = bare_entry("smith2024");
        e.title = Some("Quantum Theory".into());
        e.year = Some(2024);

        let out = match_entry(&e, &lookup).unwrap();
        assert_eq!(
            out,
            MatchOutcome::Matched("p-c".into(), MatchStrategy::TitleAndYear)
        );
    }

    #[test]
    fn ambiguous_alias_surfaces_when_no_title_year_rescue() {
        let mut lookup = MockLookup::default();
        lookup
            .by_alias
            .insert("smith2024".into(), vec!["p-a".into(), "p-b".into()]);

        let e = bare_entry("smith2024");
        let out = match_entry(&e, &lookup).unwrap();
        assert_eq!(
            out,
            MatchOutcome::AmbiguousAlias(vec!["p-a".into(), "p-b".into()])
        );
    }

    #[test]
    fn no_match_when_nothing_hits() {
        let lookup = MockLookup::default();
        let mut e = bare_entry("orphan");
        e.title = Some("Nothing".into());
        e.year = Some(1999);
        assert_eq!(match_entry(&e, &lookup).unwrap(), MatchOutcome::NoMatch);
    }
}
