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
