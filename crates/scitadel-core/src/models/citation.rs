use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{PaperId, QuestionId, SearchId, SnowballRunId};

/// Direction of a citation edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationDirection {
    References,
    CitedBy,
}

impl std::fmt::Display for CitationDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::References => write!(f, "references"),
            Self::CitedBy => write!(f, "cited_by"),
        }
    }
}

impl CitationDirection {
    /// Parse from string, matching Python's enum values.
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "references" => Some(Self::References),
            "cited_by" => Some(Self::CitedBy),
            _ => None,
        }
    }
}

/// Directed citation edge between two papers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub source_paper_id: PaperId,
    pub target_paper_id: PaperId,
    pub direction: CitationDirection,
    #[serde(default)]
    pub discovered_by: String,
    #[serde(default)]
    pub depth: i32,
    pub snowball_run_id: Option<SnowballRunId>,
}

/// Record of a snowball (citation chaining) run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnowballRun {
    pub id: SnowballRunId,
    pub search_id: Option<SearchId>,
    pub question_id: Option<QuestionId>,
    #[serde(default = "default_direction")]
    pub direction: String,
    #[serde(default = "default_max_depth")]
    pub max_depth: i32,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default)]
    pub total_discovered: i32,
    #[serde(default)]
    pub total_new_papers: i32,
    pub created_at: DateTime<Utc>,
}

fn default_direction() -> String {
    "both".to_string()
}

fn default_max_depth() -> i32 {
    1
}

fn default_threshold() -> f64 {
    0.6
}

impl SnowballRun {
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: SnowballRunId::new(),
            search_id: None,
            question_id: None,
            direction: "both".to_string(),
            max_depth: 1,
            threshold: 0.6,
            total_discovered: 0,
            total_new_papers: 0,
            created_at: Utc::now(),
        }
    }
}

impl Default for SnowballRun {
    fn default() -> Self {
        Self::new()
    }
}
