use async_trait::async_trait;
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::Client;

use scitadel_core::error::CoreError;
use scitadel_core::models::CandidatePaper;
use scitadel_core::ports::SourceAdapter;

const ESEARCH_URL: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi";
const EFETCH_URL: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi";

pub struct PubMedAdapter {
    api_key: String,
    timeout: f64,
}

impl PubMedAdapter {
    pub fn new(api_key: String, timeout: f64) -> Self {
        Self { api_key, timeout }
    }
}

#[async_trait]
impl SourceAdapter for PubMedAdapter {
    fn name(&self) -> &str {
        "pubmed"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(|e| CoreError::Adapter("pubmed".into(), e.to_string(),
            ))?;

        let pmids = esearch(&client, query, max_results, &self.api_key).await?;
        if pmids.is_empty() {
            return Ok(Vec::new());
        }
        efetch(&client, &pmids, &self.api_key).await
    }
}

async fn esearch(
    client: &Client,
    query: &str,
    max_results: usize,
    api_key: &str,
) -> Result<Vec<String>, CoreError> {
    let mut params = vec![
        ("db", "pubmed".to_string()),
        ("term", query.to_string()),
        ("retmax", max_results.to_string()),
        ("retmode", "json".to_string()),
        ("sort", "relevance".to_string()),
    ];
    if !api_key.is_empty() {
        params.push(("api_key", api_key.to_string()));
    }

    let resp = client
        .get(ESEARCH_URL)
        .query(&params)
        .send()
        .await
        .map_err(|e| CoreError::Adapter("pubmed".into(), e.to_string(),
        ))?;

    let data: serde_json::Value = resp.json().await.map_err(|e| CoreError::Adapter("pubmed".into(), e.to_string(),
    ))?;

    let ids = data
        .get("esearchresult")
        .and_then(|r| r.get("idlist"))
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(ids)
}

async fn efetch(
    client: &Client,
    pmids: &[String],
    api_key: &str,
) -> Result<Vec<CandidatePaper>, CoreError> {
    let mut params = vec![
        ("db", "pubmed".to_string()),
        ("id", pmids.join(",")),
        ("retmode", "xml".to_string()),
    ];
    if !api_key.is_empty() {
        params.push(("api_key", api_key.to_string()));
    }

    let resp = client
        .get(EFETCH_URL)
        .query(&params)
        .send()
        .await
        .map_err(|e| CoreError::Adapter("pubmed".into(), e.to_string(),
        ))?;

    let xml_text = resp.text().await.map_err(|e| CoreError::Adapter("pubmed".into(), e.to_string(),
    ))?;

    Ok(parse_pubmed_xml(&xml_text))
}

/// Parse PubMed XML response into CandidatePaper records.
pub fn parse_pubmed_xml(xml_text: &str) -> Vec<CandidatePaper> {
    let mut reader = Reader::from_str(xml_text);
    reader.config_mut().trim_text(true);

    let mut candidates = Vec::new();
    let mut rank = 0;

    // State tracking for XML parsing
    let mut in_article = false;
    let mut current_pmid = String::new();
    let mut current_title = String::new();
    let mut current_authors: Vec<String> = Vec::new();
    let mut current_abstract_parts: Vec<String> = Vec::new();
    let mut current_doi: Option<String> = None;
    let mut current_journal: Option<String> = None;
    let mut current_year: Option<i32> = None;

    // Nested element tracking
    let mut in_pmid = false;
    let mut in_article_title = false;
    let mut in_last_name = false;
    let mut in_fore_name = false;
    let mut in_abstract_text = false;
    let mut in_journal_title = false;
    let mut in_year = false;
    let mut in_elocation_id = false;
    let mut current_elocation_type = String::new();
    let mut current_last_name = String::new();
    let mut current_fore_name = String::new();
    let mut current_abstract_label = String::new();
    let mut in_medline_citation = false;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "MedlineCitation" => {
                        in_medline_citation = true;
                        in_article = false;
                        current_pmid.clear();
                        current_title.clear();
                        current_authors.clear();
                        current_abstract_parts.clear();
                        current_doi = None;
                        current_journal = None;
                        current_year = None;
                    }
                    "Article" if in_medline_citation => {
                        in_article = true;
                    }
                    "PMID" if in_medline_citation && current_pmid.is_empty() => {
                        in_pmid = true;
                    }
                    "ArticleTitle" if in_article => {
                        in_article_title = true;
                    }
                    "LastName" => {
                        in_last_name = true;
                        current_last_name.clear();
                    }
                    "ForeName" => {
                        in_fore_name = true;
                        current_fore_name.clear();
                    }
                    "AbstractText" => {
                        in_abstract_text = true;
                        current_abstract_label.clear();
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"Label" {
                                current_abstract_label =
                                    String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    "ELocationID" if in_article => {
                        in_elocation_id = true;
                        current_elocation_type.clear();
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"EIdType" {
                                current_elocation_type =
                                    String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    "Title" if in_article => {
                        in_journal_title = true;
                    }
                    "Year" if in_article => {
                        in_year = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_pmid {
                    current_pmid = text;
                } else if in_article_title {
                    current_title.push_str(&text);
                } else if in_last_name {
                    current_last_name = text;
                } else if in_fore_name {
                    current_fore_name = text;
                } else if in_abstract_text {
                    let part = if current_abstract_label.is_empty() {
                        text
                    } else {
                        format!("{}: {text}", current_abstract_label)
                    };
                    current_abstract_parts.push(part);
                } else if in_elocation_id && current_elocation_type == "doi" && current_doi.is_none()
                {
                    current_doi = Some(text);
                } else if in_journal_title {
                    current_journal = Some(text);
                } else if in_year && current_year.is_none() {
                    current_year = text.parse().ok();
                }
            }
            Ok(Event::End(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "PMID" => in_pmid = false,
                    "ArticleTitle" => in_article_title = false,
                    "LastName" => in_last_name = false,
                    "ForeName" => {
                        in_fore_name = false;
                    }
                    "Author" => {
                        if !current_last_name.is_empty() {
                            let name = if current_fore_name.is_empty() {
                                current_last_name.clone()
                            } else {
                                format!("{}, {}", current_last_name, current_fore_name)
                            };
                            current_authors.push(name);
                        }
                    }
                    "AbstractText" => in_abstract_text = false,
                    "ELocationID" => in_elocation_id = false,
                    "Title" => in_journal_title = false,
                    "Year" => in_year = false,
                    "PubmedArticle" => {
                        if !current_pmid.is_empty() {
                            rank += 1;
                            candidates.push(CandidatePaper {
                                source: "pubmed".into(),
                                source_id: current_pmid.clone(),
                                title: current_title.clone(),
                                authors: current_authors.clone(),
                                r#abstract: current_abstract_parts.join(" "),
                                doi: current_doi.clone(),
                                pubmed_id: Some(current_pmid.clone()),
                                year: current_year,
                                journal: current_journal.clone(),
                                url: Some(format!(
                                    "https://pubmed.ncbi.nlm.nih.gov/{}/",
                                    current_pmid
                                )),
                                rank: Some(rank),
                                ..CandidatePaper::new("pubmed", &current_pmid, &current_title)
                            });
                        }
                        in_medline_citation = false;
                        in_article = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!("PubMed XML parse error: {e}");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE PubmedArticleSet PUBLIC "-//NLM//DTD PubMedArticle, 1st January 2024//EN" "https://dtd.nlm.nih.gov/ncbi/pubmed/out/pubmed_240101.dtd">
<PubmedArticleSet>
  <PubmedArticle>
    <MedlineCitation>
      <PMID>12345678</PMID>
      <Article>
        <ArticleTitle>Machine Learning in Drug Discovery</ArticleTitle>
        <AuthorList>
          <Author><LastName>Smith</LastName><ForeName>John</ForeName></Author>
          <Author><LastName>Doe</LastName><ForeName>Jane</ForeName></Author>
        </AuthorList>
        <Abstract>
          <AbstractText>This paper reviews ML methods.</AbstractText>
        </Abstract>
        <ELocationID EIdType="doi">10.1234/test.2024</ELocationID>
        <Journal>
          <Title>Nature Reviews Drug Discovery</Title>
          <JournalIssue><PubDate><Year>2024</Year></PubDate></JournalIssue>
        </Journal>
      </Article>
    </MedlineCitation>
  </PubmedArticle>
</PubmedArticleSet>"#;

    #[test]
    fn test_parse_pubmed_xml() {
        let candidates = parse_pubmed_xml(SAMPLE_XML);
        assert_eq!(candidates.len(), 1);

        let c = &candidates[0];
        assert_eq!(c.source, "pubmed");
        assert_eq!(c.source_id, "12345678");
        assert_eq!(c.title, "Machine Learning in Drug Discovery");
        assert_eq!(c.authors, vec!["Smith, John", "Doe, Jane"]);
        assert_eq!(c.r#abstract, "This paper reviews ML methods.");
        assert_eq!(c.doi, Some("10.1234/test.2024".to_string()));
        assert_eq!(c.year, Some(2024));
        assert_eq!(
            c.journal,
            Some("Nature Reviews Drug Discovery".to_string())
        );
    }
}
