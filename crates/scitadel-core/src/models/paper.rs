use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::PaperId;

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paper {
    pub id: PaperId,
    pub title: String,
    pub authors: Vec<String>,
    #[serde(default)]
    pub r#abstract: String,
    pub full_text: Option<String>,
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
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: PaperId::new(),
            title: title.into(),
            authors: Vec::new(),
            r#abstract: String::new(),
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
