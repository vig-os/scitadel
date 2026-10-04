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

    /// The identity gate refused to file these bytes (#253).
    ///
    /// Boxed, and carrying a [`IdentityMismatchDetail`] rather than four fields
    /// of its own, for a mechanical reason worth stating: `AdapterError` is the
    /// error type of every ladder leg, so five inline `String`s on one variant
    /// make *every* leg's `Result` too large for `clippy::result_large_err`. The
    /// evidence is unchanged; it is just not inline.
    #[error("{0}")]
    IdentityMismatch(Box<IdentityMismatchDetail>),

    /// The dual-write of a completed download did not land. Propagated, not
    /// logged: a download the library has no row for is not a completed
    /// download.
    #[error("database error: {0}")]
    Db(#[from] scitadel_db::error::DbError),

    #[error("{0}")]
    Other(String),
}

/// The evidence behind an [`AdapterError::IdentityMismatch`].
///
/// Its own type, and public, because this is the one failure a person has to act
/// on: the two titles are the evidence ADR-007 §2's `action_list` row for
/// `identity_mismatch` promises to show ("check DOI (both titles shown)"). The
/// same pair is stored in `paper_identity_checks`, so this is a pointer to it
/// rather than the only copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityMismatchDetail {
    /// `pre_fetch` or `post_fetch`.
    pub phase: &'static str,
    /// The title we expected — the stored paper's, or a registry's.
    pub expected: String,
    /// Which side answered: the served bytes, or the registry named here.
    ///
    /// Not called `source`, because `#[error]` treats a field of that name as the
    /// error's `source()` and demands it be an `Error`.
    pub resolver: String,
    /// The title we got.
    pub resolved: String,
    /// The matcher's one-sentence reasoning, including the score.
    pub why: String,
}

impl std::fmt::Display for IdentityMismatchDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "identity mismatch: the {} check against {} failed, so nothing was filed — \
             expected {:?}, got {:?} ({}). `scitadel action_list` shows the check; \
             `scitadel override-identity` settles it if the two are the same work",
            self.phase, self.resolver, self.expected, self.resolved, self.why
        )
    }
}

impl std::error::Error for IdentityMismatchDetail {}

impl From<AdapterError> for scitadel_core::error::CoreError {
    fn from(e: AdapterError) -> Self {
        Self::Adapter("adapter".to_string(), e.to_string())
    }
}
