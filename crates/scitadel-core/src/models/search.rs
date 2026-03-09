use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{PaperId, SearchId};

/// Outcome status for a single source in a search run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    Success,
    Partial,
    Failed,
    Skipped,
}

impl std::fmt::Display for SourceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Partial => write!(f, "partial"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// Per-source result metadata for a search run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceOutcome {
    pub source: String,
    pub status: SourceStatus,
    #[serde(default)]
    pub result_count: i32,
    #[serde(default)]
    pub latency_ms: f64,
    pub error: Option<String>,
}

/// Immutable search run record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Search {
    pub id: SearchId,
    pub query: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub source_outcomes: Vec<SourceOutcome>,
    #[serde(default)]
    pub total_candidates: i32,
    #[serde(default)]
    pub total_papers: i32,
    pub created_at: DateTime<Utc>,
}

impl Search {
    #[must_use]
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            id: SearchId::new(),
            query: query.into(),
            sources: Vec::new(),
            parameters: serde_json::Value::Object(serde_json::Map::new()),
            source_outcomes: Vec::new(),
            total_candidates: 0,
            total_papers: 0,
            created_at: Utc::now(),
        }
    }
}

/// Join record: search -> paper, with per-source rank/score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub search_id: SearchId,
    pub paper_id: PaperId,
    pub source: String,
    pub rank: Option<i32>,
    pub score: Option<f64>,
    #[serde(default)]
    pub raw_metadata: serde_json::Value,
}
