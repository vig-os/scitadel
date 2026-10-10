use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::PaperId;
use crate::untrusted::{UntrustedBody, UntrustedText};

/// Canonical, deduplicated paper record.
///
/// A paper exists once regardless of how many searches found it.
///
/// # What this record does *not* carry
///
/// No `local_path`, no `download_status`, no `last_attempt_at`. Those three
/// columns (migration 007) were the pre-ADR-007 shape — one file per work,
/// recorded by absolute path, with a status column beside it. ADR-007 §1 makes
/// the database the source of truth and a work own many **artefacts**, and
/// #253's S2e stopped writing the columns.
///
/// They are gone from the type rather than left behind as dead fields on
/// purpose. A field nobody writes is the worst of both worlds: it reads back as
/// a stale truth — a `local_path` frozen at the last S1-era download — so a
/// consumer that still reads it silently reports a file that has since been
/// re-fetched, moved or deleted, with no error anywhere. Removing it turns every
/// reader into a compile error instead. The columns themselves stay until the
/// release after S2, because the S1 backfill still reads them when it reconciles
/// an existing library.
///
/// What replaces them, for every reader:
///
/// - **Which file we hold** — ADR-007 §1 "Have" (derived), through
///   `scitadel_db::sqlite::coverage::held_fulltext_artefacts`.
/// - **Whether we hold it** — the same derivation, through
///   `scitadel_db::sqlite::coverage::download_states`.
/// - **When we last tried** — `artefacts.retrieved_at` for a fetch that
///   succeeded, `acquisition_state.updated_at` for a recorded gap.
///
/// # Which fields are the document's
///
/// `title`, every entry of `authors`, `r#abstract` and `full_text` are
/// untrusted: a consumer cannot render one without going through
/// [`UntrustedText::rendered`] or [`UntrustedBody::rendered`], both of which
/// neutralise it. That is the point: #287's finding was that a crafted
/// `/Title` reached a terminal and an agent's context unescaped, and the type
/// that would have stopped it already existed. Render the field; never call
/// `as_str` on a render path.
///
/// `title` and `authors` are an [`UntrustedText`], which caps and collapses: a
/// caption is one line and every surface showing one is finite.
/// `r#abstract` and `full_text` are an [`UntrustedBody`], which caps nothing
/// and collapses nothing — a body's paragraph structure is content, and a
/// document truncated to 200 characters is a corrupted document. Surfaces that
/// show one truncate it themselves: `read_paper` takes `max_chars`, the TUI
/// reader pages, the dashboard and detail views cap with their own `truncate`.
///
/// Neither field is tagged `PublisherSupplied` when it comes back out of the
/// database. The `papers` row is scitadel's own record of the work — it was
/// stored from a feed, corrected by a human, or merged by the dedup engine — so
/// the honest provenance is [`Provenance::Ours`], the same reading
/// `IdentityCheckRow::expected_title_text` gives the identical column. That is
/// not a claim the title is safe: a feed can carry a hostile string and
/// `scitadel scan` lets a person write one, which is why `Ours` still renders
/// through `rendered()`.
///
/// A body is the exception, and it is `PublisherSupplied` at rest: there is no
/// scitadel-authored abstract or full text — a `summary` is a different field,
/// written by a model about the work — so the only honest answer to "whose is
/// this?" for a `papers.abstract` is "the document's". That is why
/// [`row_to_paper`] tags one where the title above is `Ours`: a body is *the*
/// document, the one field where a reader wants to know whose words they are
/// reading.
///
/// [`row_to_paper`]: crate::ports::PaperRepository
/// [`Provenance::Ours`]: crate::untrusted::Provenance::Ours
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paper {
    pub id: PaperId,
    pub title: UntrustedText,
    pub authors: Vec<UntrustedText>,
    #[serde(default)]
    pub r#abstract: UntrustedBody,
    pub full_text: Option<UntrustedBody>,
    pub summary: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub pubmed_id: Option<String>,
    pub inspire_id: Option<String>,
    pub openalex_id: Option<String>,
    pub year: Option<i32>,
    pub journal: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub source_urls: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Stable citation key used in BibTeX / BibLaTeX export (#132).
    /// Assigned on first encounter via the Better-BibTeX-style
    /// algorithm in `scitadel_export::bibtex::generate_key` and frozen
    /// thereafter — the freeze contract is why we persist it rather
    /// than recompute. `None` means the paper predates migration 009
    /// and will be backfilled on next `Database::migrate` call.
    #[serde(default)]
    pub bibtex_key: Option<String>,
}

impl Paper {
    /// A work with no facts but a title. `Ours`, because a freshly minted record
    /// is scitadel's own statement about a work — see the struct's docs.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: PaperId::new(),
            title: UntrustedText::ours(title),
            authors: Vec::new(),
            r#abstract: UntrustedBody::default(),
            full_text: None,
            summary: None,
            doi: None,
            arxiv_id: None,
            pubmed_id: None,
            inspire_id: None,
            openalex_id: None,
            year: None,
            journal: None,
            url: None,
            source_urls: HashMap::new(),
            created_at: now,
            updated_at: now,
            bibtex_key: None,
        }
    }

    /// This record as JSON for a surface a human or an agent reads.
    ///
    /// [`Serialize`] writes the strings exactly as stored, which is what a data
    /// path needs — `authors` is the column the DB reads back, and
    /// `export_json` is a file a person will hand to a reference manager. A
    /// *display* path needs the other thing: `scitadel show --json`,
    /// `get_paper` and `resolve_doi --json` all end up on a terminal or in an
    /// agent's context, where a publisher's `/Title` must arrive already
    /// neutralised. Both answers exist, so they are two methods rather than one
    /// that has to guess.
    ///
    /// Field-for-field the same record as `Serialize`; only the untrusted
    /// fields differ, and only in that they are rendered.
    #[must_use]
    pub fn to_display_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id.as_str(),
            "title": self.title.rendered(),
            "authors": self
                .authors
                .iter()
                .map(|author| author.rendered())
                .collect::<Vec<_>>(),
            // The body renders through `UntrustedBody::rendered`, which is the
            // security half alone: no cap, no collapsing. A 200-character cap
            // here is the bug #287's amendment deferred, and `truncate` at the
            // surface is the fix — not a cap on the type.
            "abstract": self.r#abstract.rendered(),
            "full_text": self.full_text.as_ref().map(UntrustedBody::rendered),
            "summary": self.summary,
            "doi": self.doi,
            "arxiv_id": self.arxiv_id,
            "pubmed_id": self.pubmed_id,
            "inspire_id": self.inspire_id,
            "openalex_id": self.openalex_id,
            "year": self.year,
            "journal": self.journal,
            "url": self.url,
            "source_urls": self.source_urls,
            "created_at": self.created_at.to_rfc3339(),
            "updated_at": self.updated_at.to_rfc3339(),
            "bibtex_key": self.bibtex_key,
        })
    }
}

/// Un-deduplicated paper record from a single source adapter.
///
/// Adapters produce candidates; the dedup engine merges them into Papers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidatePaper {
    pub source: String,
    pub source_id: String,
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub r#abstract: String,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub pubmed_id: Option<String>,
    pub inspire_id: Option<String>,
    pub openalex_id: Option<String>,
    pub year: Option<i32>,
    pub journal: Option<String>,
    pub url: Option<String>,
    pub rank: Option<i32>,
    pub score: Option<f64>,
    #[serde(default)]
    pub raw_data: serde_json::Value,
}

impl CandidatePaper {
    #[must_use]
    pub fn new(
        source: impl Into<String>,
        source_id: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            source_id: source_id.into(),
            title: title.into(),
            authors: Vec::new(),
            r#abstract: String::new(),
            doi: None,
            arxiv_id: None,
            pubmed_id: None,
            inspire_id: None,
            openalex_id: None,
            year: None,
            journal: None,
            url: None,
            rank: None,
            score: None,
            raw_data: serde_json::Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Serialize` is the data path and `to_display_json` the display path, and
    /// the difference is exactly the untrusted fields. A caller that picks the
    /// wrong one either corrupts a citation export or puts a publisher's
    /// bytes in an agent's context, so both answers are pinned here.
    #[test]
    fn the_two_json_forms_differ_only_in_the_untrusted_fields() {
        let mut paper = Paper::new("Deep Learning for Imaging");
        paper.authors = vec![UntrustedText::ours("Vaswani, A.")];
        paper.r#abstract = UntrustedBody::publisher_supplied("We propose a transformer.");
        paper.year = Some(2017);
        paper.doi = Some("10.1/x".into());

        let stored = serde_json::to_value(&paper).expect("serialise");
        let display = paper.to_display_json();

        assert_eq!(stored["title"], "Deep Learning for Imaging");
        assert_eq!(display["title"], "Deep Learning for Imaging");
        assert_eq!(stored["authors"], serde_json::json!(["Vaswani, A."]));
        assert_eq!(display["authors"], serde_json::json!(["Vaswani, A."]));
        // Everything else is field-for-field the same record.
        assert_eq!(stored["doi"], display["doi"]);
        assert_eq!(stored["year"], display["year"]);
        assert_eq!(stored["id"], display["id"]);
        assert_eq!(stored["abstract"], display["abstract"]);
        assert_eq!(stored["source_urls"], display["source_urls"]);

        // A hostile title is written raw by `Serialize` and neutralised by the
        // display form — and a 300-character one is capped by the display form
        // alone, which is why the data path cannot be the display path.
        let mut hostile = paper.clone();
        hostile.title = UntrustedText::publisher_supplied("T\u{1b}[2Jitle");
        hostile.authors = vec![UntrustedText::publisher_supplied("A\u{1b}[2Juthor")];
        hostile.r#abstract = UntrustedBody::publisher_supplied("Abs\u{1b}[2Jtract");
        let stored = serde_json::to_value(&hostile).expect("serialise");
        let display = hostile.to_display_json();
        assert_eq!(stored["title"], "T\u{1b}[2Jitle");
        assert_eq!(display["title"], "T itle");
        assert_eq!(display["authors"], serde_json::json!(["A uthor"]));
        assert_eq!(stored["abstract"], "Abs\u{1b}[2Jtract");
        assert_eq!(display["abstract"], "Abs tract");

        let long = "x".repeat(300);
        let mut long_paper = paper.clone();
        long_paper.title = UntrustedText::ours(&long);
        assert_eq!(
            long_paper.to_display_json()["title"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            crate::untrusted::MAX_RENDERED_CHARS
        );
        assert_eq!(
            serde_json::to_value(&long_paper).expect("serialise")["title"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            300,
            "the data form keeps the whole string"
        );
    }

    /// #287's deferred item, on the two JSON paths: a body is neutralised on the
    /// display path and left whole on the data path.
    ///
    /// The assertions that would have caught the deferral, and that a title-only
    /// wrapper could not have made: the paragraph structure survives
    /// `to_display_json`, and the body is *not* capped there. A body rendered
    /// through `UntrustedText` instead would arrive as one line, 200 characters
    /// of it.
    #[test]
    fn a_body_reaches_the_display_json_whole_and_unreshaped() {
        let raw = "First paragraph, 300 words of background.\n\nSecond paragraph, \
                   the method.\n";
        let mut paper = Paper::new("A Paper");
        paper.r#abstract = UntrustedBody::publisher_supplied(raw);
        paper.full_text = Some(UntrustedBody::publisher_supplied("Body.\n\u{1b}[2JMore."));

        let display = paper.to_display_json();
        let stored = serde_json::to_value(&paper).expect("serialise");

        assert_eq!(
            display["abstract"].as_str().expect("an abstract"),
            raw,
            "no whitespace collapsing and no length cap on the display path"
        );
        assert_eq!(
            display["full_text"].as_str().expect("a body"),
            "Body.\n More.",
            "but the escape is gone — the security half is not optional"
        );
        assert_eq!(stored["abstract"], raw, "and the data path keeps the bytes");
        assert_eq!(
            stored["full_text"], "Body.\n\u{1b}[2JMore.",
            "including the escape, because a stored column is a record"
        );
    }
}
