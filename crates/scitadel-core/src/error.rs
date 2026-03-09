use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("not found: {entity} with id {id}")]
    NotFound { entity: String, id: String },

    #[error("ambiguous prefix: {prefix} matches {count} {entity} records")]
    AmbiguousPrefix {
        entity: String,
        prefix: String,
        count: usize,
    },

    #[error("validation error: {0}")]
    Validation(String),

    #[error("adapter error: {0} — {1}")]
    Adapter(String, String),

    #[error("config error: {0}")]
    Config(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;
