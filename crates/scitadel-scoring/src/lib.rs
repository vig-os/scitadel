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
    ClaudeScorer, SCORING_SYSTEM_PROMPT, SCORING_USER_PROMPT, ScoringConfig, build_user_prompt,
    parse_scoring_response,
};
#[cfg(feature = "scoring")]
pub use cli::CliScorer;
#[cfg(feature = "scoring")]
pub use scorer::{Scorer, ScorerBackend, ScoringOptions, create_scorer};
