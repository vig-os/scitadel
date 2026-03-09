use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{QuestionId, SearchTermId};

/// First-class research question entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchQuestion {
    pub id: QuestionId,
    pub text: String,
    #[serde(default)]
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ResearchQuestion {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: QuestionId::new(),
            text: text.into(),
            description: String::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// Keyword combination linked to a research question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchTerm {
    pub id: SearchTermId,
    pub question_id: QuestionId,
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default)]
    pub query_string: String,
    pub created_at: DateTime<Utc>,
}

impl SearchTerm {
    #[must_use]
    pub fn new(question_id: QuestionId) -> Self {
        Self {
            id: SearchTermId::new(),
            question_id,
            terms: Vec::new(),
            query_string: String::new(),
            created_at: Utc::now(),
        }
    }
}
