use async_trait::async_trait;
use tracing::debug;

use scitadel_core::models::{Assessment, Paper, ResearchQuestion};

use crate::claude::{build_user_prompt, parse_scoring_response, SCORING_SYSTEM_PROMPT};
use crate::error::ScoringError;
use crate::scorer::Scorer;

/// Scorer that invokes the `claude` CLI as a subprocess.
pub struct CliScorer {
    model: String,
    temperature: f64,
}

impl CliScorer {
    pub fn new(model: String, temperature: f64) -> Self {
        Self { model, temperature }
    }
}

#[async_trait]
impl Scorer for CliScorer {
    async fn score_paper(
        &self,
        paper: &Paper,
        question: &ResearchQuestion,
    ) -> Result<Assessment, ScoringError> {
        let user_prompt = build_user_prompt(paper, question);

        let output = tokio::process::Command::new("claude")
            .arg("--print")
            .arg("--model")
            .arg(&self.model)
            .arg("--output-format")
            .arg("text")
            .arg("--system-prompt")
            .arg(SCORING_SYSTEM_PROMPT)
            .arg(&user_prompt)
            .output()
            .await
            .map_err(|e| ScoringError::Subprocess(format!("failed to spawn claude CLI: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ScoringError::Subprocess(format!(
                "claude CLI exited with {}: {}",
                output.status,
                stderr.trim()
            )));
        }

        let raw_text = String::from_utf8_lossy(&output.stdout).to_string();
        debug!(raw_len = raw_text.len(), "claude CLI response received");

        let parsed = parse_scoring_response(&raw_text);

        Ok(Assessment {
            id: scitadel_core::models::AssessmentId::new(),
            paper_id: paper.id.clone(),
            question_id: question.id.clone(),
            score: parsed.0,
            reasoning: parsed.1,
            model: Some(self.model.clone()),
            prompt: Some(user_prompt),
            temperature: Some(self.temperature),
            assessor: format!("claude-cli:{}", self.model),
            created_at: chrono::Utc::now(),
        })
    }
}
