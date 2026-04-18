use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::RwLock;

use scitadel_core::error::CoreError;
use scitadel_core::models::CandidatePaper;
use scitadel_core::ports::SourceAdapter;

const EPO_AUTH_URL: &str = "https://ops.epo.org/3.2/auth/accesstoken";
const EPO_SEARCH_URL: &str = "https://ops.epo.org/3.2/rest-services/published-data/search";

pub struct EpoOpsAdapter {
    consumer_key: String,
    consumer_secret: String,
    timeout: f64,
    /// Cached OAuth2 token with expiry.
    token: RwLock<Option<TokenData>>,
}

struct TokenData {
    access_token: String,
    expires_at: std::time::Instant,
}

impl EpoOpsAdapter {
    pub fn new(consumer_key: String, consumer_secret: String, timeout: f64) -> Self {
        Self {
            consumer_key,
            consumer_secret,
            timeout,
            token: RwLock::new(None),
        }
    }

    async fn get_token(&self, client: &Client) -> Result<String, CoreError> {
        // Check cached token
        {
            let guard = self.token.read().await;
            if let Some(ref td) = *guard {
                if td.expires_at > std::time::Instant::now() {
                    return Ok(td.access_token.clone());
                }
            }
        }

        // Fetch new token
        let resp = client
            .post(EPO_AUTH_URL)
            .basic_auth(&self.consumer_key, Some(&self.consumer_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(|e| CoreError::Adapter("epo".into(), format!("auth failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(CoreError::Adapter(
                "epo".into(),
                format!("auth HTTP {status}: {text}"),
            ));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| CoreError::Adapter("epo".into(), format!("auth parse: {e}")))?;

        let access_token = data
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CoreError::Adapter("epo".into(), "no access_token in auth response".into())
            })?
            .to_string();

        let expires_in = data
            .get("expires_in")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(1200);

        // Cache with 60s safety margin
        let expires_at = std::time::Instant::now()
            + std::time::Duration::from_secs(expires_in.saturating_sub(60));

        let mut guard = self.token.write().await;
        *guard = Some(TokenData {
            access_token: access_token.clone(),
            expires_at,
        });

        Ok(access_token)
    }
}

#[async_trait]
impl SourceAdapter for EpoOpsAdapter {
    fn name(&self) -> &str {
        "epo"
    }

    async fn search(
        &self,
        query: &str,
        _max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        if self.consumer_key.is_empty() || self.consumer_secret.is_empty() {
            return Err(CoreError::Config(
                "EPO OPS credentials not configured.\n\n\
                 To authenticate, run:\n  scitadel auth login epo\n\n\
                 Or set environment variables:\n  SCITADEL_EPO_KEY=<consumer-key>\n  SCITADEL_EPO_SECRET=<consumer-secret>\n\n\
                 Register at: https://developers.epo.org"
                    .into(),
            ));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(|e| CoreError::Adapter("epo".into(), e.to_string()))?;

        let token = self.get_token(&client).await?;

        // CQL: ta = title+abstract combined search.
        // Multi-word queries: AND each word for broader matching.
        // Single word: use directly without quotes.
        let words: Vec<&str> = query.split_whitespace().collect();
        let cql = if words.len() == 1 {
            format!("ta={}", words[0])
        } else {
            words
                .iter()
                .map(|w| format!("ta={w}"))
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        // Use /search/biblio to get inline bibliographic data.
        // Note: the standard Range header breaks text searches on EPO OPS —
        // omit it to use the default range (1-25), or use X-OPS-Range.
        let search_url = format!("{EPO_SEARCH_URL}/biblio");

        let resp = client
            .get(&search_url)
            .query(&[("q", &cql)])
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| CoreError::Adapter("epo".into(), e.to_string()))?;

        if resp.status().as_u16() == 403 {
            return Err(CoreError::Adapter(
                "epo".into(),
                "throttled by EPO fair-use policy — try again later".into(),
            ));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(CoreError::Adapter(
                "epo".into(),
                format!("HTTP {status}: {text}"),
            ));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| CoreError::Adapter("epo".into(), e.to_string()))?;

        let documents = extract_documents(&data);

        let candidates = documents
            .iter()
            .enumerate()
            .map(|(i, doc)| document_to_candidate(doc, (i + 1) as i32))
            .collect();

        Ok(candidates)
    }
}

/// Navigate the deeply nested EPO JSON to find exchange-document entries.
fn extract_documents(data: &serde_json::Value) -> Vec<serde_json::Value> {
    // Path: ops:world-patent-data > ops:biblio-search > ops:search-result > exchange-documents > exchange-document
    let search_result = data
        .pointer("/ops:world-patent-data/ops:biblio-search/ops:search-result")
        .or_else(|| data.pointer("/ops:world-patent-data/ops:biblio-search/ops:search-result"));

    let Some(result) = search_result else {
        return Vec::new();
    };

    let docs = result
        .get("exchange-documents")
        .or_else(|| result.get("ops:publication-reference"));

    let Some(docs) = docs else {
        return Vec::new();
    };

    if let Some(arr) = docs.as_array() {
        arr.iter()
            .filter_map(|d| {
                d.get("exchange-document")
                    .cloned()
                    .or_else(|| Some(d.clone()))
            })
            .collect()
    } else if let Some(doc) = docs.get("exchange-document") {
        vec![doc.clone()]
    } else {
        vec![docs.clone()]
    }
}

fn document_to_candidate(doc: &serde_json::Value, rank: i32) -> CandidatePaper {
    let country = doc.get("@country").and_then(|v| v.as_str()).unwrap_or("");
    let doc_number = doc
        .get("@doc-number")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let kind = doc.get("@kind").and_then(|v| v.as_str()).unwrap_or("");

    let patent_id = format!("{country}{doc_number}.{kind}");

    let biblio = doc
        .get("bibliographic-data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let title = extract_title(&biblio);
    let abstract_text = extract_abstract(doc);
    let inventors = extract_names(&biblio, "inventors", "inventor", "inventor-name");
    let applicants = extract_names(&biblio, "applicants", "applicant", "applicant-name");

    let pub_date = biblio
        .pointer("/publication-reference/document-id")
        .and_then(|ids| find_in_array_or_object(ids, |d| d.get("date")))
        .and_then(|d| d.get("$").or(Some(d)))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let year = if pub_date.len() >= 4 {
        pub_date[..4].parse::<i32>().ok()
    } else {
        None
    };

    let assignee = applicants.first().cloned();
    let url = format!("https://worldwide.espacenet.com/patent/search?q={country}{doc_number}");

    CandidatePaper {
        source: "epo".into(),
        source_id: patent_id.clone(),
        title,
        authors: inventors,
        r#abstract: abstract_text,
        year,
        journal: assignee.map(|a| format!("{a} ({country} {kind})")),
        url: Some(url),
        rank: Some(rank),
        raw_data: doc.clone(),
        ..CandidatePaper::new("epo", &patent_id, "")
    }
}

/// Extract title, preferring English.
fn extract_title(biblio: &serde_json::Value) -> String {
    let titles = biblio.get("invention-title");
    let Some(titles) = titles else {
        return String::new();
    };

    if let Some(arr) = titles.as_array() {
        // Prefer English title
        for t in arr {
            if t.get("@lang").and_then(|v| v.as_str()) == Some("en") {
                if let Some(text) = t.get("$").and_then(|v| v.as_str()) {
                    return text.to_string();
                }
            }
        }
        // Fallback to first
        arr.first()
            .and_then(|t| t.get("$").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string()
    } else {
        titles
            .get("$")
            .and_then(|v| v.as_str())
            .or_else(|| titles.as_str())
            .unwrap_or("")
            .to_string()
    }
}

/// Extract abstract text, preferring English.
fn extract_abstract(doc: &serde_json::Value) -> String {
    let abstracts = doc.get("abstract");
    let Some(abstracts) = abstracts else {
        return String::new();
    };

    if let Some(arr) = abstracts.as_array() {
        for a in arr {
            if a.get("@lang").and_then(|v| v.as_str()) == Some("en") {
                return extract_abstract_text(a);
            }
        }
        arr.first().map(extract_abstract_text).unwrap_or_default()
    } else {
        extract_abstract_text(abstracts)
    }
}

fn extract_abstract_text(abs: &serde_json::Value) -> String {
    abs.get("p")
        .and_then(|p| p.get("$").and_then(|v| v.as_str()).or_else(|| p.as_str()))
        .unwrap_or("")
        .to_string()
}

/// Extract names (inventors or applicants) from the parties section.
fn extract_names(
    biblio: &serde_json::Value,
    group_key: &str,
    item_key: &str,
    name_key: &str,
) -> Vec<String> {
    let group = biblio.pointer(&format!("/parties/{group_key}"));
    let Some(group) = group else {
        return Vec::new();
    };

    let items = group.get(item_key);
    let Some(items) = items else {
        return Vec::new();
    };

    let item_list: Vec<&serde_json::Value> = if let Some(arr) = items.as_array() {
        arr.iter().collect()
    } else {
        vec![items]
    };

    item_list
        .iter()
        .filter_map(|item| {
            // EPO sometimes nests as data-format="epodoc" vs "original"
            item.get(name_key)
                .and_then(|n| n.get("name"))
                .and_then(|n| n.get("$").and_then(|v| v.as_str()).or_else(|| n.as_str()))
                .map(String::from)
        })
        .collect()
}

/// Helper: navigate a value that may be a single object or an array.
fn find_in_array_or_object<F>(value: &serde_json::Value, pred: F) -> Option<&serde_json::Value>
where
    F: Fn(&serde_json::Value) -> Option<&serde_json::Value>,
{
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(v) = pred(item) {
                return Some(v);
            }
        }
        None
    } else {
        pred(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_to_candidate() {
        let doc = serde_json::json!({
            "@country": "EP",
            "@doc-number": "1234567",
            "@kind": "A1",
            "bibliographic-data": {
                "invention-title": [
                    {"@lang": "de", "$": "Ein Verfahren"},
                    {"@lang": "en", "$": "A method for something"}
                ],
                "parties": {
                    "inventors": {
                        "inventor": [
                            {"inventor-name": {"name": {"$": "DOE, Jane"}}},
                            {"inventor-name": {"name": {"$": "SMITH, John"}}}
                        ]
                    },
                    "applicants": {
                        "applicant": {
                            "applicant-name": {"name": {"$": "ACME Corp"}}
                        }
                    }
                },
                "publication-reference": {
                    "document-id": {
                        "date": {"$": "20230315"}
                    }
                }
            },
            "abstract": [
                {"@lang": "en", "p": {"$": "This invention relates to something useful."}}
            ]
        });

        let c = document_to_candidate(&doc, 1);
        assert_eq!(c.source, "epo");
        assert_eq!(c.source_id, "EP1234567.A1");
        assert_eq!(c.title, "A method for something");
        assert_eq!(c.r#abstract, "This invention relates to something useful.");
        assert_eq!(c.year, Some(2023));
        assert_eq!(c.authors, vec!["DOE, Jane", "SMITH, John"]);
        assert_eq!(c.journal, Some("ACME Corp (EP A1)".to_string()));
        assert_eq!(c.rank, Some(1));
    }

    #[test]
    fn test_document_to_candidate_minimal() {
        let doc = serde_json::json!({
            "@country": "US",
            "@doc-number": "9999999",
            "@kind": "B2",
        });

        let c = document_to_candidate(&doc, 5);
        assert_eq!(c.source_id, "US9999999.B2");
        assert!(c.title.is_empty());
        assert!(c.authors.is_empty());
        assert_eq!(c.rank, Some(5));
    }

    #[test]
    fn test_extract_title_single() {
        let biblio = serde_json::json!({
            "invention-title": {"$": "A single title"}
        });
        assert_eq!(extract_title(&biblio), "A single title");
    }

    #[test]
    fn test_extract_title_prefers_english() {
        let biblio = serde_json::json!({
            "invention-title": [
                {"@lang": "fr", "$": "Un titre"},
                {"@lang": "en", "$": "A title"}
            ]
        });
        assert_eq!(extract_title(&biblio), "A title");
    }
}
