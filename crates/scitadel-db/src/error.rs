use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("migration error: {0}")]
    Migration(String),

    #[error("core error: {0}")]
    Core(#[from] scitadel_core::error::CoreError),
}

impl From<DbError> for scitadel_core::error::CoreError {
    fn from(e: DbError) -> Self {
        Self::Adapter("db".to_string(), e.to_string())
    }
}
