mod annotation;
mod assessment;
mod citation;
mod doi;
mod paper;
mod question;
mod search;

pub use annotation::{
    Anchor, AnchorStatus, Annotation, AnnotationRead, normalize_sentence, sentence_id,
};
pub use assessment::Assessment;
pub use citation::{Citation, CitationDirection, SnowballRun};
pub use doi::{doi_to_filename, normalize_doi, validate_doi};
pub use paper::{CandidatePaper, Paper};
pub use question::{ResearchQuestion, SearchTerm};
pub use search::{Search, SearchResult, SourceOutcome, SourceStatus};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Newtype wrapper for type-safe IDs.
macro_rules! newtype_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4().simple().to_string())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn short(&self) -> &str {
                &self.0[..self.0.len().min(8)]
            }

            #[must_use]
            pub fn starts_with(&self, prefix: &str) -> bool {
                self.0.starts_with(prefix)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

newtype_id!(PaperId);
newtype_id!(SearchId);
newtype_id!(QuestionId);
newtype_id!(AssessmentId);
newtype_id!(SearchTermId);
newtype_id!(SnowballRunId);
newtype_id!(AnnotationId);
