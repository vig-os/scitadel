use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Audit trail for a scoring operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringProvenance {
    pub model: String,
    pub temperature: f64,
    pub prompt: String,
    pub raw_response: String,
    pub parsed_score: f64,
    pub parsed_reasoning: String,
    pub timestamp: DateTime<Utc>,
}
