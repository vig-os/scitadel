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

    #[error("{0}")]
    Other(String),
}

impl From<AdapterError> for scitadel_core::error::CoreError {
    fn from(e: AdapterError) -> Self {
        Self::Adapter("adapter".to_string(), e.to_string())
    }
}
