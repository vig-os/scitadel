use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::PaperId;

/// Canonical, deduplicated paper record.
///
/// A paper exists once regardless of how many searches found it.
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
    /// Absolute path to the locally downloaded file (PDF/HTML), if any.
    /// Populated by the download pipeline; `None` until first successful
    /// download attempt. See #112.
    #[serde(default)]
    pub local_path: Option<String>,
    /// Outcome of the most recent download attempt. `None` = never tried.
    #[serde(default)]
    pub download_status: Option<DownloadStatus>,
    /// Wall-clock time of the most recent download attempt. Together with
    /// `download_status` lets the UI distinguish "fresh failure" from
    /// "tried weeks ago, retry might work".
    #[serde(default)]
    pub last_attempt_at: Option<DateTime<Utc>>,
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
            local_path: None,
            download_status: None,
            last_attempt_at: None,
        }
    }
}

/// Outcome of a paper download attempt. Persisted on `papers.download_status`.
///
/// `Downloaded` means the adapter classified the fetched bytes as full
/// content. `Paywall` means we got bytes but they're an HTML stub /
/// abstract / paywall page — file exists but doesn't contain the paper.
/// `Failed` means the download itself errored (network, 404, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Downloaded,
    Paywall,
    Failed,
}

impl DownloadStatus {
    /// SQL-friendly string used in the `download_status` text column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Downloaded => "downloaded",
            Self::Paywall => "paywall",
            Self::Failed => "failed",
        }
    }

    /// Inverse of `as_str`. Returns `None` for unknown values so a
    /// stale row from a future schema doesn't crash the loader.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "downloaded" => Some(Self::Downloaded),
            "paywall" => Some(Self::Paywall),
            "failed" => Some(Self::Failed),
            _ => None,
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
