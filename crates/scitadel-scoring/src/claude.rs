use async_trait::async_trait;
use reqwest::Client;
use tracing::{info, warn};

use scitadel_core::models::{Assessment, Paper, ResearchQuestion};

use crate::error::ScoringError;
use crate::scorer::Scorer;

pub const SCORING_SYSTEM_PROMPT: &str = "\
You are a scientific literature relevance assessor. You evaluate how relevant \
a paper is to a specific research question.

Score on a scale of 0.0 to 1.0:
- 0.0-0.2: Not relevant — different topic, no connection
- 0.2-0.4: Tangentially relevant — related field but doesn't address the question
- 0.4-0.6: Moderately relevant — partially addresses the question or related methodology
- 0.6-0.8: Relevant — directly addresses aspects of the question
- 0.8-1.0: Highly relevant — core paper for this research question

Respond with valid JSON only: {\"score\": float, \"reasoning\": \"string\"}
The reasoning should be 1-3 sentences explaining your assessment.";

pub const SCORING_USER_PROMPT: &str = "\
Research Question: {question_text}
{question_description}

Paper Title: {title}
Authors: {authors}
Year: {year}
Journal: {journal}
Abstract: {abstract}

Rate the relevance of this paper to the research question.";

/// Configuration for Claude-based scoring.
#[derive(Debug, Clone)]
pub struct ScoringConfig {
    pub model: String,
    pub temperature: f64,
    pub max_tokens: u32,
    pub api_key: String,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-6".to_string(),
            temperature: 0.0,
            max_tokens: 512,
            api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        }
    }
}

/// Claude-based paper relevance scorer.
pub struct ClaudeScorer {
    client: Client,
    config: ScoringConfig,
}

impl ClaudeScorer {
    pub fn new(config: ScoringConfig) -> Self {
        let client = Client::new();
        Self { client, config }
    }

    /// Score a single paper against a research question.
    pub async fn score_paper(
        &self,
        paper: &Paper,
        question: &ResearchQuestion,
    ) -> Result<Assessment, ScoringError> {
        let user_prompt = build_user_prompt(paper, question);

        let body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
            "system": SCORING_SYSTEM_PROMPT,
            "messages": [
                {"role": "user", "content": user_prompt}
            ]
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let data: serde_json::Value = resp.json().await?;

        let raw_text = data
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        let parsed = parse_scoring_response(&raw_text);

        Ok(Assessment {
            id: scitadel_core::models::AssessmentId::new(),
            paper_id: paper.id.clone(),
            question_id: question.id.clone(),
            score: parsed.0,
            reasoning: parsed.1,
            model: Some(self.config.model.clone()),
            prompt: Some(user_prompt),
            temperature: Some(self.config.temperature),
            assessor: self.config.model.clone(),
            created_at: chrono::Utc::now(),
        })
    }

    /// Score multiple papers with optional progress callback.
    pub async fn score_papers(
        &self,
        papers: &[Paper],
        question: &ResearchQuestion,
        on_progress: Option<&dyn Fn(usize, usize, &Paper, &Assessment)>,
    ) -> Vec<Assessment> {
        let mut assessments = Vec::new();

        for (i, paper) in papers.iter().enumerate() {
            match self.score_paper(paper, question).await {
                Ok(assessment) => {
                    info!(
                        paper_idx = i + 1,
                        total = papers.len(),
                        score = assessment.score,
                        title = %paper.title.chars().take(60).collect::<String>(),
                        "Scored paper"
                    );
                    if let Some(cb) = on_progress {
                        cb(i, papers.len(), paper, &assessment);
                    }
                    assessments.push(assessment);
                }
                Err(e) => {
                    warn!(
                        paper_idx = i + 1,
                        total = papers.len(),
                        error = %e,
                        "Failed to score paper"
                    );
                    assessments.push(Assessment {
                        id: scitadel_core::models::AssessmentId::new(),
                        paper_id: paper.id.clone(),
                        question_id: question.id.clone(),
                        score: 0.0,
                        reasoning: format!("Scoring failed: {e}"),
                        model: Some(self.config.model.clone()),
                        prompt: None,
                        temperature: Some(self.config.temperature),
                        assessor: format!("{}:error", self.config.model),
                        created_at: chrono::Utc::now(),
                    });
                }
            }
        }

        assessments
    }
}

pub fn build_user_prompt(paper: &Paper, question: &ResearchQuestion) -> String {
    let description = if question.description.is_empty() {
        String::new()
    } else {
        format!("Context: {}", question.description)
    };

    let authors = paper.authors.iter().take(5).cloned().collect::<Vec<_>>().join("; ");
    let abstract_text = if paper.r#abstract.len() > 2000 {
        &paper.r#abstract[..2000]
    } else if paper.r#abstract.is_empty() {
        "No abstract available."
    } else {
        &paper.r#abstract
    };

    SCORING_USER_PROMPT
        .replace("{question_text}", &question.text)
        .replace("{question_description}", &description)
        .replace("{title}", &paper.title)
        .replace("{authors}", &authors)
        .replace("{year}", &paper.year.map_or_else(|| "N/A".into(), |y| y.to_string()))
        .replace("{journal}", paper.journal.as_deref().unwrap_or("N/A"))
        .replace("{abstract}", abstract_text)
}

pub fn parse_scoring_response(text: &str) -> (f64, String) {
    let text = text.trim();

    // Handle markdown code blocks
    let cleaned = if text.starts_with("```") {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() > 2 {
            lines[1..lines.len() - 1].join("\n")
        } else {
            text.to_string()
        }
    } else {
        text.to_string()
    };

    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&cleaned) {
        let score = data
            .get("score")
            .and_then(|s| s.as_f64())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let reasoning = data
            .get("reasoning")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        (score, reasoning)
    } else {
        warn!("Failed to parse scoring response: {}", &text[..text.len().min(200)]);
        (0.0, format!("Parse error. Raw response: {}", &text[..text.len().min(500)]))
    }
}

#[async_trait]
impl Scorer for ClaudeScorer {
    async fn score_paper(
        &self,
        paper: &Paper,
        question: &ResearchQuestion,
    ) -> Result<Assessment, ScoringError> {
        self.score_paper(paper, question).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scoring_response_valid() {
        let (score, reasoning) =
            parse_scoring_response(r#"{"score": 0.85, "reasoning": "Highly relevant paper."}"#);
        assert!((score - 0.85).abs() < f64::EPSILON);
        assert_eq!(reasoning, "Highly relevant paper.");
    }

    #[test]
    fn test_parse_scoring_response_clamping() {
        let (score, _) = parse_scoring_response(r#"{"score": 1.5, "reasoning": "test"}"#);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_scoring_response_invalid() {
        let (score, reasoning) = parse_scoring_response("not json");
        assert!((score - 0.0).abs() < f64::EPSILON);
        assert!(reasoning.contains("Parse error"));
    }

    #[test]
    fn test_parse_scoring_response_markdown() {
        let (score, _) =
            parse_scoring_response("```json\n{\"score\": 0.7, \"reasoning\": \"test\"}\n```");
        assert!((score - 0.7).abs() < f64::EPSILON);
    }
}
