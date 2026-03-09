use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScoringError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("API error: {0}")]
    Api(String),

    #[error("parse error: {0}")]
    Parse(String),
}

impl From<ScoringError> for scitadel_core::error::CoreError {
    fn from(e: ScoringError) -> Self {
        Self::Adapter("scoring".to_string(), e.to_string())
    }
}
