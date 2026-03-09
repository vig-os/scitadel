use async_trait::async_trait;
use reqwest::Client;

use scitadel_core::error::CoreError;
use scitadel_core::models::CandidatePaper;
use scitadel_core::ports::SourceAdapter;

const OPENALEX_API_URL: &str = "https://api.openalex.org/works";

pub struct OpenAlexAdapter {
    email: String,
    timeout: f64,
}

impl OpenAlexAdapter {
    pub fn new(email: String, timeout: f64) -> Self {
        Self { email, timeout }
    }
}

#[async_trait]
impl SourceAdapter for OpenAlexAdapter {
    fn name(&self) -> &str {
        "openalex"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(|e| CoreError::Adapter("openalex".into(), e.to_string(),
            ))?;

        let mut params = vec![
            ("search", query.to_string()),
            ("per_page", max_results.to_string()),
        ];
        if !self.email.is_empty() {
            params.push(("mailto", self.email.clone()));
        }

        let resp = client
            .get(OPENALEX_API_URL)
            .query(&params)
            .send()
            .await
            .map_err(|e| CoreError::Adapter("openalex".into(), e.to_string(),
            ))?;

        let data: serde_json::Value = resp.json().await.map_err(|e| CoreError::Adapter("openalex".into(), e.to_string(),
        ))?;

        let works = data
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();

        let candidates = works
            .iter()
            .enumerate()
            .map(|(i, work)| work_to_candidate(work, (i + 1) as i32))
            .collect();

        Ok(candidates)
    }
}

fn work_to_candidate(work: &serde_json::Value, rank: i32) -> CandidatePaper {
    let openalex_id = work.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let short_id = openalex_id.rsplit('/').next().unwrap_or("").to_string();

    let title = work
        .get("title")
        .or_else(|| work.get("display_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let authors: Vec<String> = work
        .get("authorships")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    a.get("author")
                        .and_then(|au| au.get("display_name"))
                        .and_then(|n| n.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    let abstract_text = work
        .get("abstract_inverted_index")
        .and_then(|v| v.as_object())
        .map(reconstruct_abstract)
        .unwrap_or_default();

    let doi_url = work.get("doi").and_then(|v| v.as_str()).unwrap_or("");
    let doi = if doi_url.is_empty() {
        None
    } else {
        Some(doi_url.replace("https://doi.org/", ""))
    };

    let year = work
        .get("publication_year")
        .and_then(|v| v.as_i64())
        .map(|y| y as i32);

    let journal = work
        .get("primary_location")
        .and_then(|loc| loc.get("source"))
        .and_then(|src| src.get("display_name"))
        .and_then(|n| n.as_str())
        .map(String::from);

    let pmid = work
        .get("ids")
        .and_then(|ids| ids.get("pmid"))
        .and_then(|v| v.as_str())
        .and_then(|url| url.rsplit('/').next())
        .map(String::from);

    CandidatePaper {
        source: "openalex".into(),
        source_id: short_id.clone(),
        title: title.clone(),
        authors,
        r#abstract: abstract_text,
        doi,
        openalex_id: Some(short_id),
        pubmed_id: pmid,
        year,
        journal,
        url: Some(openalex_id.to_string()),
        rank: Some(rank),
        raw_data: work.clone(),
        ..CandidatePaper::new("openalex", "", &title)
    }
}

/// Reconstruct abstract text from OpenAlex inverted index format.
pub fn reconstruct_abstract(inverted_index: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut word_positions: Vec<(i64, &str)> = Vec::new();

    for (word, positions) in inverted_index {
        if let Some(arr) = positions.as_array() {
            for pos in arr {
                if let Some(p) = pos.as_i64() {
                    word_positions.push((p, word));
                }
            }
        }
    }

    word_positions.sort_by_key(|(pos, _)| *pos);
    word_positions
        .iter()
        .map(|(_, word)| *word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert an OpenAlex work dict to Paper constructor kwargs (for citation fetching).
pub fn work_to_paper_dict(work: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let candidate = work_to_candidate(work, 0);
    let mut map = serde_json::Map::new();
    map.insert("title".into(), serde_json::Value::String(candidate.title));
    map.insert(
        "authors".into(),
        serde_json::Value::Array(
            candidate
                .authors
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    map.insert(
        "abstract".into(),
        serde_json::Value::String(candidate.r#abstract),
    );
    if let Some(doi) = candidate.doi {
        map.insert("doi".into(), serde_json::Value::String(doi));
    }
    if let Some(id) = candidate.openalex_id {
        map.insert("openalex_id".into(), serde_json::Value::String(id));
    }
    if let Some(pmid) = candidate.pubmed_id {
        map.insert("pubmed_id".into(), serde_json::Value::String(pmid));
    }
    if let Some(year) = candidate.year {
        map.insert("year".into(), serde_json::Value::Number(year.into()));
    }
    if let Some(journal) = candidate.journal {
        map.insert("journal".into(), serde_json::Value::String(journal));
    }
    if let Some(url) = candidate.url {
        map.insert("url".into(), serde_json::Value::String(url));
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconstruct_abstract() {
        let mut index = serde_json::Map::new();
        index.insert(
            "Hello".into(),
            serde_json::Value::Array(vec![serde_json::Value::Number(0.into())]),
        );
        index.insert(
            "world".into(),
            serde_json::Value::Array(vec![serde_json::Value::Number(1.into())]),
        );

        let result = reconstruct_abstract(&index);
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_work_to_candidate() {
        let work = serde_json::json!({
            "id": "https://openalex.org/W1234567890",
            "title": "Test Paper",
            "doi": "https://doi.org/10.1234/test",
            "publication_year": 2024,
            "authorships": [
                {"author": {"display_name": "Alice Smith"}},
                {"author": {"display_name": "Bob Jones"}}
            ],
            "primary_location": {
                "source": {"display_name": "Nature"}
            }
        });

        let c = work_to_candidate(&work, 1);
        assert_eq!(c.source, "openalex");
        assert_eq!(c.title, "Test Paper");
        assert_eq!(c.doi, Some("10.1234/test".to_string()));
        assert_eq!(c.year, Some(2024));
        assert_eq!(c.authors, vec!["Alice Smith", "Bob Jones"]);
        assert_eq!(c.journal, Some("Nature".to_string()));
    }
}
