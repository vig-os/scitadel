use async_trait::async_trait;
use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::Client;

use scitadel_core::error::CoreError;
use scitadel_core::models::CandidatePaper;
use scitadel_core::ports::SourceAdapter;

const ARXIV_API_URL: &str = "https://export.arxiv.org/api/query";

pub struct ArxivAdapter {
    timeout: f64,
}

impl ArxivAdapter {
    pub fn new(timeout: f64) -> Self {
        Self { timeout }
    }
}

#[async_trait]
impl SourceAdapter for ArxivAdapter {
    fn name(&self) -> &str {
        "arxiv"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(|e| CoreError::Adapter("arxiv".into(), e.to_string()))?;

        let params = [
            ("search_query", format!("all:{query}")),
            ("start", "0".to_string()),
            ("max_results", max_results.to_string()),
            ("sortBy", "relevance".to_string()),
            ("sortOrder", "descending".to_string()),
        ];

        let resp = client
            .get(ARXIV_API_URL)
            .query(&params)
            .send()
            .await
            .map_err(|e| CoreError::Adapter("arxiv".into(), e.to_string()))?;

        let xml_text = resp
            .text()
            .await
            .map_err(|e| CoreError::Adapter("arxiv".into(), e.to_string()))?;

        Ok(parse_arxiv_atom(&xml_text))
    }
}

/// Extract arXiv ID from URL like `http://arxiv.org/abs/2301.12345v1`.
fn extract_arxiv_id(url: &str) -> String {
    if let Some(pos) = url.find("arxiv.org/abs/") {
        let id_part = &url[pos + 14..];
        // Strip version suffix
        if let Some(v_pos) = id_part.rfind('v')
            && id_part[v_pos + 1..].chars().all(|c| c.is_ascii_digit())
        {
            return id_part[..v_pos].to_string();
        }
        return id_part.to_string();
    }
    url.to_string()
}

/// Parse arXiv Atom XML response into CandidatePaper records.
pub fn parse_arxiv_atom(xml_text: &str) -> Vec<CandidatePaper> {
    let mut reader = Reader::from_str(xml_text);
    reader.config_mut().trim_text(true);

    let mut candidates = Vec::new();
    let mut rank = 0;

    // State tracking
    let mut in_entry = false;
    let mut current_id = String::new();
    let mut current_title = String::new();
    let mut current_summary = String::new();
    let mut current_authors: Vec<String> = Vec::new();
    let mut current_doi: Option<String> = None;
    let mut current_year: Option<i32> = None;
    let mut current_journal: Option<String> = None;

    let mut in_id = false;
    let mut in_title = false;
    let mut in_summary = false;
    let mut in_author_name = false;
    let mut in_published = false;
    let mut depth = 0; // track entry nesting
    let mut entry_depth = 0;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let local_name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                match local_name.as_str() {
                    "entry" => {
                        in_entry = true;
                        entry_depth = depth;
                        current_id.clear();
                        current_title.clear();
                        current_summary.clear();
                        current_authors.clear();
                        current_doi = None;
                        current_year = None;
                        current_journal = None;
                    }
                    "id" if in_entry => in_id = true,
                    "title" if in_entry => in_title = true,
                    "summary" if in_entry => in_summary = true,
                    "name" if in_entry => in_author_name = true,
                    "published" if in_entry => in_published = true,
                    "doi" if in_entry => {
                        // arxiv:doi element — read text in next event
                        in_id = false; // reuse a flag approach
                    }
                    "journal_ref" if in_entry => {}
                    _ => {}
                }

                // Check for arxiv:doi and arxiv:journal_ref via namespace prefix
                let full_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if full_name.contains(":doi") && in_entry {
                    // Will capture text in next Text event
                    // Use a special flag
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_id {
                    current_id.push_str(&text);
                } else if in_title {
                    current_title.push_str(&text);
                } else if in_summary {
                    current_summary.push_str(&text);
                } else if in_author_name {
                    current_authors.push(text);
                } else if in_published && current_year.is_none() && text.len() >= 4 {
                    current_year = text[..4].parse().ok();
                }
            }
            Ok(Event::End(ref e)) => {
                let local_name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                match local_name.as_str() {
                    "id" => in_id = false,
                    "title" => in_title = false,
                    "summary" => in_summary = false,
                    "name" => in_author_name = false,
                    "published" => in_published = false,
                    "entry" if depth == entry_depth => {
                        in_entry = false;
                        if !current_id.is_empty() {
                            rank += 1;
                            let arxiv_id = extract_arxiv_id(&current_id);
                            // Collapse whitespace
                            let title: String = current_title
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");
                            let abstract_text: String = current_summary
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");

                            candidates.push(CandidatePaper {
                                source: "arxiv".into(),
                                source_id: arxiv_id.clone(),
                                title: title.clone(),
                                authors: current_authors.clone(),
                                r#abstract: abstract_text,
                                doi: current_doi.clone(),
                                arxiv_id: Some(arxiv_id),
                                year: current_year,
                                journal: current_journal.clone(),
                                url: Some(current_id.clone()),
                                rank: Some(rank),
                                ..CandidatePaper::new("arxiv", "", &title)
                            });
                        }
                    }
                    _ => {}
                }
                depth -= 1;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!("arXiv XML parse error: {e}");
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

    #[test]
    fn test_extract_arxiv_id() {
        assert_eq!(
            extract_arxiv_id("http://arxiv.org/abs/2301.12345v1"),
            "2301.12345"
        );
        assert_eq!(
            extract_arxiv_id("http://arxiv.org/abs/2301.12345"),
            "2301.12345"
        );
    }

    const SAMPLE_ATOM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <entry>
    <id>http://arxiv.org/abs/2301.00001v2</id>
    <title>Quantum Machine Learning: A Survey</title>
    <summary>We review recent advances in quantum ML.</summary>
    <author><name>Alice Smith</name></author>
    <author><name>Bob Jones</name></author>
    <published>2023-01-01T00:00:00Z</published>
  </entry>
</feed>"#;

    #[test]
    fn test_parse_arxiv_atom() {
        let candidates = parse_arxiv_atom(SAMPLE_ATOM);
        assert_eq!(candidates.len(), 1);

        let c = &candidates[0];
        assert_eq!(c.source, "arxiv");
        assert_eq!(c.source_id, "2301.00001");
        assert_eq!(c.title, "Quantum Machine Learning: A Survey");
        assert_eq!(c.authors, vec!["Alice Smith", "Bob Jones"]);
        assert_eq!(c.year, Some(2023));
    }
}
