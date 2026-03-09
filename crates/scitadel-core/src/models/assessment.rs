use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{AssessmentId, PaperId, QuestionId};

/// Paper x research question -> relevance score + provenance.
///
/// Multiple assessments per paper (different questions, models, human override).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    pub id: AssessmentId,
    pub paper_id: PaperId,
    pub question_id: QuestionId,
    pub score: f64,
    #[serde(default)]
    pub reasoning: String,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub temperature: Option<f64>,
    #[serde(default)]
    pub assessor: String,
    pub created_at: DateTime<Utc>,
}

impl Assessment {
    #[must_use]
    pub fn new(paper_id: PaperId, question_id: QuestionId, score: f64) -> Self {
        Self {
            id: AssessmentId::new(),
            paper_id,
            question_id,
            score,
            reasoning: String::new(),
            model: None,
            prompt: None,
            temperature: None,
            assessor: String::new(),
            created_at: Utc::now(),
        }
    }
}
