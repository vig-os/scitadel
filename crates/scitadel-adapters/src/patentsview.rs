use async_trait::async_trait;
use reqwest::Client;

use scitadel_core::error::CoreError;
use scitadel_core::models::CandidatePaper;
use scitadel_core::ports::SourceAdapter;

/// PatentSearch API v1 (successor to the discontinued PatentsView query API).
/// Note: This endpoint migrates to USPTO Open Data Portal (data.uspto.gov) in 2026.
const PATENTSVIEW_API_URL: &str = "https://search.patentsview.org/api/v1/patent/";

pub struct PatentsViewAdapter {
    api_key: String,
    timeout: f64,
}

impl PatentsViewAdapter {
    pub fn new(api_key: String, timeout: f64) -> Self {
        Self { api_key, timeout }
    }
}

#[async_trait]
impl SourceAdapter for PatentsViewAdapter {
    fn name(&self) -> &str {
        "patentsview"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        if self.api_key.is_empty() {
            return Err(CoreError::Config(
                "PatentsView API key required.\n\n\
                 To authenticate, run:\n  scitadel auth login patentsview\n\n\
                 Or set environment variable:\n  SCITADEL_PATENTSVIEW_KEY=<your-key>\n\n\
                 Request a key at: https://patentsview.org/apis/purpose"
                    .into(),
            ));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(|e| CoreError::Adapter("patentsview".into(), e.to_string()))?;

        let size = max_results.min(1000);
        let body = serde_json::json!({
            "q": { "_text_any": { "patent_abstract": query } },
            "f": [
                "patent_id",
                "patent_title",
                "patent_abstract",
                "patent_date",
                "patent_type",
                "patent_kind",
                "assignees.assignee_organization",
                "inventors.inventor_name_first",
                "inventors.inventor_name_last",
            ],
            "o": {
                "size": size,
            },
            "s": [{ "patent_date": "desc" }],
        });

        let resp = client
            .post(PATENTSVIEW_API_URL)
            .header("X-Api-Key", &self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::Adapter("patentsview".into(), e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(CoreError::Adapter(
                "patentsview".into(),
                format!("HTTP {status}: {text}"),
            ));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| CoreError::Adapter("patentsview".into(), e.to_string()))?;

        let patents = data
            .get("patents")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();

        let candidates = patents
            .iter()
            .enumerate()
            .map(|(i, patent)| patent_to_candidate(patent, (i + 1) as i32))
            .collect();

        Ok(candidates)
    }
}

fn patent_to_candidate(patent: &serde_json::Value, rank: i32) -> CandidatePaper {
    let patent_id = patent
        .get("patent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let title = patent
        .get("patent_title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let abstract_text = patent
        .get("patent_abstract")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let year = patent
        .get("patent_date")
        .and_then(|v| v.as_str())
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<i32>().ok());

    let inventors = extract_inventors(patent);

    let assignee = patent
        .get("assignees")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|a| a.get("assignee_organization"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let patent_type = patent
        .get("patent_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let patent_kind = patent
        .get("patent_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let url = format!("https://patents.google.com/patent/US{patent_id}");

    CandidatePaper {
        source: "patentsview".into(),
        source_id: patent_id.to_string(),
        title,
        authors: inventors,
        r#abstract: abstract_text,
        year,
        journal: assignee.map(|a| format!("{a} ({patent_type} {patent_kind})")),
        url: Some(url),
        rank: Some(rank),
        raw_data: patent.clone(),
        ..CandidatePaper::new("patentsview", patent_id, "")
    }
}

/// Extract inventor names from the nested inventors array.
fn extract_inventors(patent: &serde_json::Value) -> Vec<String> {
    patent
        .get("inventors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|inv| {
                    let first = inv
                        .get("inventor_name_first")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let last = inv
                        .get("inventor_name_last")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if first.is_empty() && last.is_empty() {
                        None
                    } else {
                        Some(format!("{first} {last}").trim().to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patent_to_candidate() {
        let patent = serde_json::json!({
            "patent_id": "11234567",
            "patent_title": "Method for quantum error correction",
            "patent_abstract": "A method for correcting errors in quantum computing systems.",
            "patent_date": "2023-01-31",
            "patent_type": "utility",
            "patent_kind": "B2",
            "inventors": [
                {"inventor_name_first": "Alice", "inventor_name_last": "Smith"},
                {"inventor_name_first": "Bob", "inventor_name_last": "Jones"}
            ],
            "assignees": [
                {"assignee_organization": "Quantum Corp"}
            ]
        });

        let c = patent_to_candidate(&patent, 1);
        assert_eq!(c.source, "patentsview");
        assert_eq!(c.source_id, "11234567");
        assert_eq!(c.title, "Method for quantum error correction");
        assert_eq!(c.year, Some(2023));
        assert_eq!(c.authors, vec!["Alice Smith", "Bob Jones"]);
        assert_eq!(c.journal, Some("Quantum Corp (utility B2)".to_string()));
        assert_eq!(
            c.url,
            Some("https://patents.google.com/patent/US11234567".to_string())
        );
        assert_eq!(c.rank, Some(1));
    }

    #[test]
    fn test_extract_inventors_empty() {
        let patent = serde_json::json!({});
        assert!(extract_inventors(&patent).is_empty());
    }

    #[test]
    fn test_patent_to_candidate_minimal() {
        let patent = serde_json::json!({
            "patent_id": "9999999",
            "patent_title": "Something",
        });

        let c = patent_to_candidate(&patent, 3);
        assert_eq!(c.source_id, "9999999");
        assert_eq!(c.title, "Something");
        assert_eq!(c.year, None);
        assert!(c.authors.is_empty());
        assert_eq!(c.rank, Some(3));
    }
}
