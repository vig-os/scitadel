use async_trait::async_trait;
use reqwest::Client;

use scitadel_core::error::CoreError;
use scitadel_core::models::CandidatePaper;
use scitadel_core::ports::SourceAdapter;

const INSPIRE_API_URL: &str = "https://inspirehep.net/api/literature";

pub struct InspireAdapter {
    timeout: f64,
}

impl InspireAdapter {
    pub fn new(timeout: f64) -> Self {
        Self { timeout }
    }
}

#[async_trait]
impl SourceAdapter for InspireAdapter {
    fn name(&self) -> &str {
        "inspire"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(|e| CoreError::Adapter("inspire".into(), e.to_string(),
            ))?;

        let params = [
            ("q", query.to_string()),
            ("size", max_results.to_string()),
            ("sort", "mostrecent".to_string()),
            (
                "fields",
                "titles,authors,abstracts,dois,arxiv_eprints,publication_info".to_string(),
            ),
        ];

        let resp = client
            .get(INSPIRE_API_URL)
            .query(&params)
            .send()
            .await
            .map_err(|e| CoreError::Adapter("inspire".into(), e.to_string(),
            ))?;

        let data: serde_json::Value = resp.json().await.map_err(|e| CoreError::Adapter("inspire".into(), e.to_string(),
        ))?;

        Ok(parse_inspire_results(&data))
    }
}

pub fn parse_inspire_results(data: &serde_json::Value) -> Vec<CandidatePaper> {
    let hits = data
        .get("hits")
        .and_then(|h| h.get("hits"))
        .and_then(|h| h.as_array())
        .cloned()
        .unwrap_or_default();

    hits.iter()
        .enumerate()
        .map(|(i, hit)| {
            let meta = hit.get("metadata").cloned().unwrap_or_default();
            let inspire_id = hit
                .get("id")
                .and_then(|v| v.as_i64().map(|n| n.to_string()).or_else(|| v.as_str().map(String::from)))
                .unwrap_or_default();

            let title = meta
                .get("titles")
                .and_then(|t| t.as_array())
                .and_then(|arr| arr.first())
                .and_then(|t| t.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let authors: Vec<String> = meta
                .get("authors")
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.get("full_name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let abstract_text = meta
                .get("abstracts")
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let doi = meta
                .get("dois")
                .and_then(|d| d.as_array())
                .and_then(|arr| arr.first())
                .and_then(|d| d.get("value"))
                .and_then(|v| v.as_str())
                .map(String::from);

            let arxiv_id = meta
                .get("arxiv_eprints")
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.get("value"))
                .and_then(|v| v.as_str())
                .map(String::from);

            let pub_info = meta
                .get("publication_info")
                .and_then(|p| p.as_array())
                .and_then(|arr| arr.first());

            let year = pub_info
                .and_then(|p| p.get("year"))
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .map(|y| y as i32);

            let journal = pub_info
                .and_then(|p| p.get("journal_title"))
                .and_then(|v| v.as_str())
                .map(String::from);

            CandidatePaper {
                source: "inspire".into(),
                source_id: inspire_id.clone(),
                title: title.clone(),
                authors,
                r#abstract: abstract_text,
                doi,
                arxiv_id,
                inspire_id: Some(inspire_id.clone()),
                year,
                journal,
                url: Some(format!("https://inspirehep.net/literature/{inspire_id}")),
                rank: Some((i + 1) as i32),
                ..CandidatePaper::new("inspire", &inspire_id, &title)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_inspire_results() {
        let data = serde_json::json!({
            "hits": {
                "hits": [
                    {
                        "id": 123456,
                        "metadata": {
                            "titles": [{"title": "Higgs Boson Discovery"}],
                            "authors": [
                                {"full_name": "Atlas Collaboration"}
                            ],
                            "abstracts": [{"value": "We report the observation..."}],
                            "dois": [{"value": "10.1016/test"}],
                            "arxiv_eprints": [{"value": "1207.7214"}],
                            "publication_info": [
                                {"year": 2012, "journal_title": "Phys. Lett. B"}
                            ]
                        }
                    }
                ]
            }
        });

        let candidates = parse_inspire_results(&data);
        assert_eq!(candidates.len(), 1);

        let c = &candidates[0];
        assert_eq!(c.source, "inspire");
        assert_eq!(c.title, "Higgs Boson Discovery");
        assert_eq!(c.doi, Some("10.1016/test".to_string()));
        assert_eq!(c.arxiv_id, Some("1207.7214".to_string()));
        assert_eq!(c.year, Some(2012));
    }
}
