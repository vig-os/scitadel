use async_trait::async_trait;
use tracing::warn;

use scitadel_core::models::{Assessment, Paper, ResearchQuestion};

use crate::error::ScoringError;

/// Abstraction over different scoring backends (API, CLI subprocess, etc.).
#[async_trait]
pub trait Scorer: Send + Sync {
    /// Score a single paper against a research question.
    async fn score_paper(
        &self,
        paper: &Paper,
        question: &ResearchQuestion,
    ) -> Result<Assessment, ScoringError>;

    /// Score multiple papers with optional progress callback.
    ///
    /// Default implementation calls `score_paper` in a loop.
    async fn score_papers(
        &self,
        papers: &[Paper],
        question: &ResearchQuestion,
    ) -> Vec<Assessment> {
        let mut assessments = Vec::new();
        let total = papers.len();

        for (i, paper) in papers.iter().enumerate() {
            match self.score_paper(paper, question).await {
                Ok(assessment) => {
                    assessments.push(assessment);
                }
                Err(e) => {
                    warn!(
                        paper_idx = i + 1,
                        total,
                        error = %e,
                        "Failed to score paper"
                    );
                    assessments.push(Assessment {
                        id: scitadel_core::models::AssessmentId::new(),
                        paper_id: paper.id.clone(),
                        question_id: question.id.clone(),
                        score: 0.0,
                        reasoning: format!("Scoring failed: {e}"),
                        model: None,
                        prompt: None,
                        temperature: None,
                        assessor: "error".to_string(),
                        created_at: chrono::Utc::now(),
                    });
                }
            }
        }

        assessments
    }
}

/// Which scoring backend to use.
pub enum ScorerBackend {
    /// Automatically detect: prefer CLI, fall back to API.
    Auto,
    /// Use `claude` CLI subprocess.
    Cli,
    /// Use Anthropic HTTP API directly.
    Api,
}

/// Options for creating a scorer.
pub struct ScoringOptions {
    pub backend: ScorerBackend,
    pub model: String,
    pub temperature: f64,
}

/// Create a scorer based on the requested backend.
///
/// For `Auto`: checks if the `claude` CLI is available, prefers `CliScorer`,
/// falls back to `ClaudeScorer` (API key).
pub async fn create_scorer(options: ScoringOptions) -> Result<Box<dyn Scorer>, ScoringError> {
    match options.backend {
        ScorerBackend::Cli => {
            if !cli_available().await {
                return Err(ScoringError::Subprocess(
                    "claude CLI not found in PATH".to_string(),
                ));
            }
            Ok(Box::new(crate::cli::CliScorer::new(
                options.model,
                options.temperature,
            )))
        }
        ScorerBackend::Api => {
            let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
                ScoringError::Api("ANTHROPIC_API_KEY environment variable not set".to_string())
            })?;
            let config = crate::claude::ScoringConfig {
                model: options.model,
                temperature: options.temperature,
                api_key,
                ..Default::default()
            };
            Ok(Box::new(crate::claude::ClaudeScorer::new(config)))
        }
        ScorerBackend::Auto => {
            if cli_available().await {
                Ok(Box::new(crate::cli::CliScorer::new(
                    options.model,
                    options.temperature,
                )))
            } else if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                let config = crate::claude::ScoringConfig {
                    model: options.model,
                    temperature: options.temperature,
                    api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
                    ..Default::default()
                };
                Ok(Box::new(crate::claude::ClaudeScorer::new(config)))
            } else {
                Err(ScoringError::Api(
                    "no scorer available: claude CLI not found and ANTHROPIC_API_KEY not set"
                        .to_string(),
                ))
            }
        }
    }
}

/// Check whether the `claude` CLI binary is available on PATH.
async fn cli_available() -> bool {
    tokio::process::Command::new("which")
        .arg("claude")
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}
