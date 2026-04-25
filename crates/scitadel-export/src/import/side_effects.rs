//! Zotero compat — non-row work that follows a successful import
//! resolution (#134 step 4):
//!
//! - `note={...}` → an unanchored [`Annotation`] (paper-level), with
//!   the bib's `keywords={a,b,c}` carried as the annotation's tags.
//!   Author is the import-time `reader`, matching the convention used
//!   for TUI / MCP annotation writes.
//! - `keywords=` *without* a `note=` are surfaced for `--verbose`
//!   logging — paper-level tags don't exist as first-class data yet
//!   (a tracked gap; a follow-up issue would add a `paper_tags`
//!   table). Dropping silently would lose user data without warning.
//! - `file={...}` is always surfaced for `--verbose` logging — the
//!   issue spec deliberately drops these, since paths in someone
//!   else's Zotero export are meaningless on the importer's machine.
//!
//! The alias side effect is unconditional for all non-Rejected
//! actions: the imported citekey gets recorded on `paper_aliases`
//! (#134 step 1) so a future re-import resolves via the alias step
//! of the match cascade.

use scitadel_core::models::{Anchor, Annotation, PaperId};

use super::merge::MergeAction;
use super::parse::BibEntry;

/// `source` value recorded on `paper_aliases` rows that originate
/// here — mirrors the constant in `scitadel_db::sqlite::SOURCE_BIBTEX_IMPORT`.
/// Re-declared instead of imported to keep `scitadel-export` free of a
/// `scitadel-db` dependency.
pub const ALIAS_SOURCE: &str = "bibtex-import";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasRecord {
    pub paper_id: String,
    pub alias: String,
    pub source: &'static str,
}

#[derive(Debug, Clone)]
pub struct SideEffects {
    /// Citekey-as-alias to record. `None` only when the merge action
    /// was `Rejected` (no DB writes happen at all).
    pub alias: Option<AliasRecord>,
    /// `note=` carrying user's reading context. Built unanchored
    /// (paper-level), with `bib.keywords` attached as tags.
    pub annotation: Option<Annotation>,
    /// `keywords=` we couldn't attach to anything because there was
    /// no `note=` to anchor them on. Surface in `--verbose` so the
    /// user knows their tags didn't make it.
    pub dropped_keywords: Vec<String>,
    /// `file=` paths — never imported (paths from a foreign machine
    /// are meaningless), but surfaced in `--verbose`.
    pub dropped_file: Option<String>,
}

impl SideEffects {
    /// Empty bundle — used when `MergeAction::Rejected` short-circuits
    /// the side-effect plumbing entirely.
    pub fn rejected() -> Self {
        Self {
            alias: None,
            annotation: None,
            dropped_keywords: vec![],
            dropped_file: None,
        }
    }

    /// True when no DB writes nor user-facing logs follow from this
    /// import row — the caller can short-circuit per-row work.
    pub fn is_empty(&self) -> bool {
        self.alias.is_none()
            && self.annotation.is_none()
            && self.dropped_keywords.is_empty()
            && self.dropped_file.is_none()
    }
}

/// Compute side effects for an import row. `paper_id` is the resolved
/// paper id (post-match or post-create); `reader` is the import-time
/// identity used as `Annotation.author` to mirror TUI/MCP conventions.
pub fn compute(paper_id: &str, reader: &str, bib: &BibEntry, action: MergeAction) -> SideEffects {
    if action == MergeAction::Rejected {
        return SideEffects::rejected();
    }

    let alias = Some(AliasRecord {
        paper_id: paper_id.to_string(),
        alias: bib.citekey.clone(),
        source: ALIAS_SOURCE,
    });

    let (annotation, dropped_keywords) = match bib.note.as_deref() {
        Some(note) if !note.is_empty() => {
            let mut a = Annotation::new_root(
                PaperId::from(paper_id),
                reader.to_string(),
                note.to_string(),
                Anchor::default(),
            );
            a.tags.clone_from(&bib.keywords);
            (Some(a), vec![])
        }
        _ => (None, bib.keywords.clone()),
    };

    SideEffects {
        alias,
        annotation,
        dropped_keywords,
        dropped_file: bib.file.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn bib(citekey: &str) -> BibEntry {
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
            extra: HashMap::new(),
        }
    }

    #[test]
    fn rejected_action_yields_empty_side_effects() {
        let mut b = bib("k");
        b.note = Some("ignored".into());
        b.keywords = vec!["x".into()];
        b.file = Some("/p.pdf".into());
        let se = compute("p1", "lars", &b, MergeAction::Rejected);
        assert!(se.is_empty());
        assert!(se.alias.is_none());
    }

    #[test]
    fn alias_recorded_for_every_non_rejected_action() {
        for action in [
            MergeAction::Created,
            MergeAction::Updated,
            MergeAction::Unchanged,
        ] {
            let se = compute("p1", "lars", &bib("smith2024"), action);
            let a = se.alias.expect("alias recorded");
            assert_eq!(a.paper_id, "p1");
            assert_eq!(a.alias, "smith2024");
            assert_eq!(a.source, "bibtex-import");
        }
    }

    #[test]
    fn note_becomes_unanchored_annotation_with_reader_as_author() {
        let mut b = bib("k");
        b.note = Some("Read twice; methodology questionable".into());
        let se = compute("p-x", "lars", &b, MergeAction::Updated);
        let a = se.annotation.expect("annotation built from note");
        assert_eq!(a.author, "lars");
        assert_eq!(a.note, "Read twice; methodology questionable");
        assert_eq!(a.paper_id.as_str(), "p-x");
        assert!(a.parent_id.is_none(), "root annotation");
        assert_eq!(
            a.anchor,
            Anchor::default(),
            "unanchored — no source text yet"
        );
    }

    #[test]
    fn keywords_attach_to_annotation_when_note_exists() {
        let mut b = bib("k");
        b.note = Some("ok".into());
        b.keywords = vec!["alpha".into(), "beta".into()];
        let se = compute("p1", "lars", &b, MergeAction::Updated);
        let a = se.annotation.unwrap();
        assert_eq!(a.tags, vec!["alpha", "beta"]);
        assert!(se.dropped_keywords.is_empty());
    }

    #[test]
    fn keywords_without_note_are_dropped_for_verbose_log() {
        let mut b = bib("k");
        b.keywords = vec!["alpha".into(), "beta".into()];
        let se = compute("p1", "lars", &b, MergeAction::Updated);
        assert!(se.annotation.is_none());
        assert_eq!(se.dropped_keywords, vec!["alpha", "beta"]);
    }

    #[test]
    fn empty_note_string_is_treated_as_no_note() {
        let mut b = bib("k");
        b.note = Some(String::new());
        b.keywords = vec!["x".into()];
        let se = compute("p1", "lars", &b, MergeAction::Updated);
        assert!(
            se.annotation.is_none(),
            "empty note string must not produce an empty-content annotation"
        );
        assert_eq!(se.dropped_keywords, vec!["x"]);
    }

    #[test]
    fn file_field_always_surfaces_under_verbose() {
        let mut b = bib("k");
        b.file = Some("/somebody/elses/path.pdf".into());
        let se = compute("p1", "lars", &b, MergeAction::Created);
        assert_eq!(se.dropped_file.as_deref(), Some("/somebody/elses/path.pdf"));
    }

    #[test]
    fn no_zotero_extras_yields_alias_only() {
        let se = compute("p1", "lars", &bib("k"), MergeAction::Updated);
        assert!(se.alias.is_some());
        assert!(se.annotation.is_none());
        assert!(se.dropped_keywords.is_empty());
        assert!(se.dropped_file.is_none());
    }
}
