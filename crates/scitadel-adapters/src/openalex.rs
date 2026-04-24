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

    /// Fetch the full Work JSON for a single paper by its short OpenAlex
    /// id (e.g. `W2741809807`). Returns the raw API payload so callers
    /// can pluck whatever fields they need (title, `referenced_works`,
    /// authorships, etc.). Used by the citation-graph service (#59).
    pub async fn fetch_work_by_id(
        &self,
        openalex_id: &str,
    ) -> Result<serde_json::Value, CoreError> {
        self.fetch_from(&format!("{OPENALEX_API_URL}/{openalex_id}"), &[])
            .await
    }

    /// Fetch a batch of Works by their short OpenAlex ids (W…) in one
    /// request. OpenAlex's `openalex_id:W1|W2|…` filter accepts up to
    /// 50 entries per call; the caller is responsible for chunking
    /// larger lists. Returns the deserialised `results` array.
    pub async fn fetch_works_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<serde_json::Value>, CoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if ids.len() > 50 {
            return Err(CoreError::Adapter(
                "openalex".into(),
                format!(
                    "fetch_works_by_ids requires <=50 ids per call, got {}",
                    ids.len()
                ),
            ));
        }
        let filter = format!("openalex_id:{}", ids.join("|"));
        let payload = self
            .fetch_from(
                OPENALEX_API_URL,
                &[("filter", filter.as_str()), ("per_page", "50")],
            )
            .await?;
        Ok(payload
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Fetch Works that cite the given paper (the reverse-citation
    /// direction). `limit` defaults to 25 and is capped at 200 by the
    /// OpenAlex API.
    pub async fn fetch_cited_by(
        &self,
        openalex_id: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, CoreError> {
        let filter = format!("cites:{openalex_id}");
        let per_page = limit.clamp(1, 200).to_string();
        let payload = self
            .fetch_from(
                OPENALEX_API_URL,
                &[("filter", filter.as_str()), ("per_page", per_page.as_str())],
            )
            .await?;
        Ok(payload
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    async fn fetch_from(
        &self,
        url: &str,
        extra: &[(&str, &str)],
    ) -> Result<serde_json::Value, CoreError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(|e| CoreError::Adapter("openalex".into(), e.to_string()))?;

        let mut params: Vec<(&str, String)> =
            extra.iter().map(|(k, v)| (*k, (*v).to_string())).collect();
        if !self.email.is_empty() {
            params.push(("mailto", self.email.clone()));
        }

        let resp = client
            .get(url)
            .query(&params)
            .send()
            .await
            .map_err(|e| CoreError::Adapter("openalex".into(), e.to_string()))?;
        if !resp.status().is_success() {
            return Err(CoreError::Adapter(
                "openalex".into(),
                format!("HTTP {} for {url}", resp.status()),
            ));
        }
        resp.json()
            .await
            .map_err(|e| CoreError::Adapter("openalex".into(), e.to_string()))
    }
}

/// Extract the short OpenAlex id (`W2741809807`) from either a full URL
/// (`https://openalex.org/W2741809807`) or a bare id. Returns `None` if
/// the input doesn't end in a `W…` token.
#[must_use]
pub fn short_openalex_id(maybe_url: &str) -> Option<String> {
    let last = maybe_url.rsplit('/').next().unwrap_or(maybe_url);
    if last.starts_with('W') && last.len() > 1 {
        Some(last.to_string())
    } else {
        None
    }
}

/// Build a `Paper` (canonical record) from an OpenAlex Work JSON.
/// Useful when materialising referenced works as DB rows.
#[must_use]
pub fn work_to_paper(work: &serde_json::Value) -> scitadel_core::models::Paper {
    use scitadel_core::models::{Paper, PaperId};
    let candidate = work_to_candidate(work, 0);
    let mut paper = Paper::new(candidate.title);
    if !candidate.authors.is_empty() {
        paper.authors = candidate.authors;
    }
    paper.r#abstract = candidate.r#abstract;
    paper.doi = candidate.doi;
    paper.openalex_id.clone_from(&candidate.openalex_id);
    paper.pubmed_id = candidate.pubmed_id;
    paper.year = candidate.year;
    paper.journal = candidate.journal;
    paper.url = candidate.url;
    if let Some(short) = candidate.openalex_id {
        // Use the short OpenAlex id as the canonical paper id so
        // citation rows have a stable target_paper_id even when the
        // metadata gets re-fetched later.
        paper.id = PaperId::from(short);
    }
    paper
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
            .map_err(|e| CoreError::Adapter("openalex".into(), e.to_string()))?;

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
            .map_err(|e| CoreError::Adapter("openalex".into(), e.to_string()))?;

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| CoreError::Adapter("openalex".into(), e.to_string()))?;

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

    #[test]
    fn short_openalex_id_extracts_from_url_or_bare_id() {
        assert_eq!(
            short_openalex_id("https://openalex.org/W2741809807"),
            Some("W2741809807".into())
        );
        assert_eq!(short_openalex_id("W2741809807"), Some("W2741809807".into()));
        assert_eq!(short_openalex_id("not-a-work"), None);
        assert_eq!(short_openalex_id("https://openalex.org/A12345"), None);
        // Edge: just "W" alone is not a valid id.
        assert_eq!(short_openalex_id("W"), None);
    }

    #[test]
    fn work_to_paper_uses_openalex_id_as_canonical_paper_id() {
        let work = serde_json::json!({
            "id": "https://openalex.org/W2741809807",
            "title": "Foundational paper",
            "publication_year": 2017,
            "doi": "https://doi.org/10.5555/foo",
        });
        let paper = work_to_paper(&work);
        assert_eq!(paper.id.as_str(), "W2741809807");
        assert_eq!(paper.openalex_id.as_deref(), Some("W2741809807"));
        assert_eq!(paper.title, "Foundational paper");
        assert_eq!(paper.year, Some(2017));
        assert_eq!(paper.doi.as_deref(), Some("10.5555/foo"));
    }
}
