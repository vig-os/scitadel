#[cfg(feature = "scoring")]
pub mod claude;
pub mod error;
pub mod provenance;

#[cfg(feature = "scoring")]
pub use claude::{
    build_user_prompt, parse_scoring_response, ClaudeScorer, ScoringConfig,
    SCORING_SYSTEM_PROMPT, SCORING_USER_PROMPT,
};
