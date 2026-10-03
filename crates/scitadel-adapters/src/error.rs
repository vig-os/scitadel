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

    /// A paced fetch that failed outright (ADR-007 §4). Kept typed rather
    /// than flattened into [`Self::Network`] because `needs_login`,
    /// `is_rate_limited` and `retry_at_ms` are decisions a caller makes on
    /// the *variant*, and collapsing them loses exactly the distinctions the
    /// `FetchError` split exists for.
    #[error("paced fetch failed: {0}")]
    Paced(#[from] scitadel_http::FetchError),

    /// The dual-write of a completed download did not land. Propagated, not
    /// logged: a download the library has no row for is not a completed
    /// download.
    #[error("database error: {0}")]
    Db(#[from] scitadel_db::error::DbError),

    #[error("{0}")]
    Other(String),
}

impl From<AdapterError> for scitadel_core::error::CoreError {
    fn from(e: AdapterError) -> Self {
        Self::Adapter("adapter".to_string(), e.to_string())
    }
}
