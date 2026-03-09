#[cfg(feature = "scoring")]
pub mod claude;
#[cfg(feature = "scoring")]
pub mod cli;
pub mod error;
pub mod provenance;
#[cfg(feature = "scoring")]
pub mod scorer;

#[cfg(feature = "scoring")]
pub use claude::{
    build_user_prompt, parse_scoring_response, ClaudeScorer, ScoringConfig,
    SCORING_SYSTEM_PROMPT, SCORING_USER_PROMPT,
};
#[cfg(feature = "scoring")]
pub use cli::CliScorer;
#[cfg(feature = "scoring")]
pub use scorer::{create_scorer, Scorer, ScorerBackend, ScoringOptions};
