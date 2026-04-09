use async_trait::async_trait;
use reqwest::Client;

use scitadel_core::error::CoreError;
use scitadel_core::models::CandidatePaper;
use scitadel_core::ports::SourceAdapter;

const LENS_API_URL: &str = "https://api.lens.org/patent/search";

pub struct LensAdapter {
    api_token: String,
    timeout: f64,
}

impl LensAdapter {
    pub fn new(api_token: String, timeout: f64) -> Self {
        Self { api_token, timeout }
    }
}

#[async_trait]
impl SourceAdapter for LensAdapter {
    fn name(&self) -> &str {
        "lens"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        if self.api_token.is_empty() {
            return Err(CoreError::Config(
                "Lens.org credentials not configured.\n\n\
                 To authenticate, run:\n  scitadel auth login lens\n\n\
                 Or set environment variable:\n  SCITADEL_LENS_TOKEN=<your-token>\n\n\
                 Get a token at: https://www.lens.org/lens/user/subscriptions"
                    .into(),
            ));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(|e| CoreError::Adapter("lens".into(), e.to_string()))?;

        let size = max_results.min(100); // Lens max per request
        let body = serde_json::json!({
            "query": {
                "bool": {
                    "must": [
                        { "match": { "title": query } }
                    ]
                }
            },
            "size": size,
            "from": 0,
            "include": [
                "lens_id",
                "doc_number",
                "title",
                "abstract",
                "date_published",
                "biblio.parties.inventors",
                "biblio.parties.applicants",
                "biblio.references.npl_cit",
            ],
            "sort": [{ "date_published": "desc" }],
        });

        let resp = client
            .post(LENS_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::Adapter("lens".into(), e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(CoreError::Adapter(
                "lens".into(),
                format!("HTTP {status}: {text}"),
            ));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| CoreError::Adapter("lens".into(), e.to_string()))?;

        let patents = data
            .get("data")
            .and_then(|d| d.as_array())
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
    let lens_id = patent
        .get("lens_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let doc_number = patent
        .get("doc_number")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let title = patent
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let abstract_text = patent
        .get("abstract")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let year = patent
        .get("date_published")
        .and_then(|v| v.as_str())
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<i32>().ok());

    let inventors = extract_inventors(patent);

    let applicant = patent
        .pointer("/biblio/parties/applicants")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .map(String::from);

    // Extract non-patent literature citations (links patents to scholarly papers)
    let npl_citations = extract_npl_citations(patent);

    let source_id = if !lens_id.is_empty() {
        lens_id.to_string()
    } else {
        doc_number.to_string()
    };

    let url = if !lens_id.is_empty() {
        Some(format!("https://www.lens.org/lens/patent/{lens_id}"))
    } else {
        None
    };

    let mut raw = patent.clone();
    if !npl_citations.is_empty() {
        if let Some(obj) = raw.as_object_mut() {
            obj.insert(
                "npl_citations".into(),
                serde_json::Value::Array(
                    npl_citations
                        .iter()
                        .map(|c| serde_json::Value::String(c.clone()))
                        .collect(),
                ),
            );
        }
    }

    CandidatePaper {
        source: "lens".into(),
        source_id,
        title,
        authors: inventors,
        r#abstract: abstract_text,
        year,
        journal: applicant,
        url,
        rank: Some(rank),
        raw_data: raw,
        ..CandidatePaper::new("lens", "", "")
    }
}

fn extract_inventors(patent: &serde_json::Value) -> Vec<String> {
    patent
        .pointer("/biblio/parties/inventors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|inv| inv.get("name").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract non-patent literature (NPL) citation texts.
///
/// These are references from patents to scholarly papers — the key value
/// of Lens.org for connecting patent and academic literature.
fn extract_npl_citations(patent: &serde_json::Value) -> Vec<String> {
    patent
        .pointer("/biblio/references/npl_cit")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|cit| {
                    cit.get("text")
                        .or_else(|| cit.as_str().map(|_| cit))
                        .and_then(|v| v.as_str())
                        .map(String::from)
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
            "lens_id": "031-698-123-456-789",
            "doc_number": "US20210001234A1",
            "title": "Methods for CRISPR gene editing",
            "abstract": "The present invention relates to CRISPR systems.",
            "date_published": "2021-01-07",
            "biblio": {
                "parties": {
                    "inventors": [
                        {"name": "Jane Doe", "country": "US"},
                        {"name": "John Smith", "country": "US"}
                    ],
                    "applicants": [
                        {"name": "Acme Corp", "country": "US"}
                    ]
                },
                "references": {
                    "npl_cit": [
                        {"text": "Doudna et al., Science, 2014", "lens_id": "009-442-876-222-111"}
                    ]
                }
            }
        });

        let c = patent_to_candidate(&patent, 1);
        assert_eq!(c.source, "lens");
        assert_eq!(c.source_id, "031-698-123-456-789");
        assert_eq!(c.title, "Methods for CRISPR gene editing");
        assert_eq!(c.year, Some(2021));
        assert_eq!(c.authors, vec!["Jane Doe", "John Smith"]);
        assert_eq!(c.journal, Some("Acme Corp".to_string()));
        assert_eq!(
            c.url,
            Some("https://www.lens.org/lens/patent/031-698-123-456-789".to_string())
        );

        // NPL citations stored in raw_data
        let npl = c.raw_data.get("npl_citations").unwrap().as_array().unwrap();
        assert_eq!(npl.len(), 1);
        assert_eq!(npl[0].as_str().unwrap(), "Doudna et al., Science, 2014");
    }

    #[test]
    fn test_patent_to_candidate_minimal() {
        let patent = serde_json::json!({
            "doc_number": "EP1234567",
            "title": "Something",
        });

        let c = patent_to_candidate(&patent, 2);
        assert_eq!(c.source_id, "EP1234567");
        assert_eq!(c.title, "Something");
        assert!(c.authors.is_empty());
        assert_eq!(c.url, None);
    }

    #[test]
    fn test_extract_npl_citations_empty() {
        let patent = serde_json::json!({});
        assert!(extract_npl_citations(&patent).is_empty());
    }
}
