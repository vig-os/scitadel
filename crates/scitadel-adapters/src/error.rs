use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::DeError),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unknown source: {0}")]
    UnknownSource(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("I/O error: {0}")]
    Io(String),

    #[error("{0}")]
    Other(String),
}

impl From<AdapterError> for scitadel_core::error::CoreError {
    fn from(e: AdapterError) -> Self {
        Self::Adapter("adapter".to_string(), e.to_string())
    }
}
