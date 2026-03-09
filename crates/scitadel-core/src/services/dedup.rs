use std::collections::HashMap;

use crate::models::{CandidatePaper, Paper, SearchResult, SearchId};

/// Normalize title for fuzzy matching: lowercase, strip punctuation/whitespace.
fn normalize_title(title: &str) -> String {
    let lowered = title.to_lowercase();
    let stripped: String = lowered
        .chars()
        .map(|c: char| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Simple word-overlap similarity (Jaccard) for title matching.
fn title_similarity(a: &str, b: &str) -> f64 {
    let norm_a = normalize_title(a);
    let norm_b = normalize_title(b);
    let words_a: std::collections::HashSet<String> =
        norm_a.split_whitespace().map(String::from).collect();
    let words_b: std::collections::HashSet<String> =
        norm_b.split_whitespace().map(String::from).collect();

    if words_a.is_empty() || words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();
    intersection as f64 / union as f64
}

/// Merge a candidate's metadata into an existing paper (fill gaps).
fn merge_candidate_into_paper(paper: &mut Paper, candidate: &CandidatePaper) {
    if paper.doi.is_none() && candidate.doi.is_some() {
        paper.doi.clone_from(&candidate.doi);
    }
    if paper.arxiv_id.is_none() && candidate.arxiv_id.is_some() {
        paper.arxiv_id.clone_from(&candidate.arxiv_id);
    }
    if paper.pubmed_id.is_none() && candidate.pubmed_id.is_some() {
        paper.pubmed_id.clone_from(&candidate.pubmed_id);
    }
    if paper.inspire_id.is_none() && candidate.inspire_id.is_some() {
        paper.inspire_id.clone_from(&candidate.inspire_id);
    }
    if paper.openalex_id.is_none() && candidate.openalex_id.is_some() {
        paper.openalex_id.clone_from(&candidate.openalex_id);
    }
    if paper.r#abstract.is_empty() && !candidate.r#abstract.is_empty() {
        paper.r#abstract.clone_from(&candidate.r#abstract);
    }
    if paper.year.is_none() && candidate.year.is_some() {
        paper.year = candidate.year;
    }
    if paper.journal.is_none() && candidate.journal.is_some() {
        paper.journal.clone_from(&candidate.journal);
    }
    if paper.authors.is_empty() && !candidate.authors.is_empty() {
        paper.authors.clone_from(&candidate.authors);
    }
    if let Some(url) = &candidate.url {
        paper.source_urls.insert(candidate.source.clone(), url.clone());
    }
}

/// Deduplicate candidates into canonical Papers.
///
/// Returns `(papers, search_results)` — deduplicated papers and per-source
/// search result records with provenance.
pub fn deduplicate(
    candidates: &[CandidatePaper],
    title_threshold: f64,
) -> (Vec<Paper>, Vec<SearchResult>) {
    let mut doi_index: HashMap<String, usize> = HashMap::new();
    let mut title_index: HashMap<String, usize> = HashMap::new();
    let mut papers: Vec<Paper> = Vec::new();
    let mut search_results: Vec<SearchResult> = Vec::new();

    for candidate in candidates {
        let mut matched_idx = None;

        // 1. DOI exact match
        if let Some(doi) = &candidate.doi {
            let doi_lower = doi.to_lowercase();
            if let Some(&idx) = doi_index.get(&doi_lower) {
                matched_idx = Some(idx);
            }
        }

        // 2. Fuzzy title match (only if no DOI match)
        if matched_idx.is_none() && !candidate.title.is_empty() {
            let norm_title = normalize_title(&candidate.title);
            if let Some(&idx) = title_index.get(&norm_title) {
                matched_idx = Some(idx);
            } else {
                for &idx in title_index.values() {
                    if title_similarity(&candidate.title, &papers[idx].title) >= title_threshold {
                        matched_idx = Some(idx);
                        break;
                    }
                }
            }
        }

        if let Some(idx) = matched_idx {
            merge_candidate_into_paper(&mut papers[idx], candidate);
        } else {
            let mut paper = Paper::new(&candidate.title);
            paper.authors.clone_from(&candidate.authors);
            paper.r#abstract.clone_from(&candidate.r#abstract);
            paper.doi.clone_from(&candidate.doi);
            paper.arxiv_id.clone_from(&candidate.arxiv_id);
            paper.pubmed_id.clone_from(&candidate.pubmed_id);
            paper.inspire_id.clone_from(&candidate.inspire_id);
            paper.openalex_id.clone_from(&candidate.openalex_id);
            paper.year = candidate.year;
            paper.journal.clone_from(&candidate.journal);
            paper.url.clone_from(&candidate.url);
            if let Some(url) = &candidate.url {
                paper.source_urls.insert(candidate.source.clone(), url.clone());
            }

            let idx = papers.len();
            matched_idx = Some(idx);

            if let Some(doi) = &candidate.doi {
                doi_index.insert(doi.to_lowercase(), idx);
            }
            if !candidate.title.is_empty() {
                title_index.insert(normalize_title(&candidate.title), idx);
            }

            papers.push(paper);
        }

        let idx = matched_idx.unwrap();
        search_results.push(SearchResult {
            search_id: SearchId::from(""),
            paper_id: papers[idx].id.clone(),
            source: candidate.source.clone(),
            rank: candidate.rank,
            score: candidate.score,
            raw_metadata: candidate.raw_data.clone(),
        });
    }

    (papers, search_results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_title() {
        assert_eq!(normalize_title("Hello, World!"), "hello world");
        assert_eq!(normalize_title("  Multiple   Spaces  "), "multiple spaces");
    }

    #[test]
    fn test_title_similarity_identical() {
        let sim = title_similarity("Machine Learning for Science", "Machine Learning for Science");
        assert!((sim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_title_similarity_different() {
        let sim = title_similarity("Quantum Computing", "Baking Recipes");
        assert!(sim < 0.5);
    }

    #[test]
    fn test_dedup_by_doi() {
        let c1 = CandidatePaper {
            doi: Some("10.1234/test".into()),
            ..CandidatePaper::new("pubmed", "1", "Test Paper")
        };
        let c2 = CandidatePaper {
            doi: Some("10.1234/test".into()),
            arxiv_id: Some("2301.00001".into()),
            ..CandidatePaper::new("arxiv", "2", "Test Paper")
        };

        let (papers, results) = deduplicate(&[c1, c2], 0.85);
        assert_eq!(papers.len(), 1);
        assert_eq!(results.len(), 2);
        assert!(papers[0].arxiv_id.is_some(), "should merge arxiv_id");
    }

    #[test]
    fn test_dedup_by_title_similarity() {
        let c1 = CandidatePaper::new("pubmed", "1", "Deep Learning for Drug Discovery");
        let c2 = CandidatePaper::new("arxiv", "2", "Deep Learning for Drug Discovery: A Review");

        let (papers, _) = deduplicate(&[c1, c2], 0.7);
        // With threshold 0.7, these should merge (high word overlap)
        assert_eq!(papers.len(), 1);
    }

    #[test]
    fn test_dedup_distinct_papers() {
        let c1 = CandidatePaper::new("pubmed", "1", "Quantum Computing Advances");
        let c2 = CandidatePaper::new("arxiv", "2", "Machine Learning in Biology");

        let (papers, _) = deduplicate(&[c1, c2], 0.85);
        assert_eq!(papers.len(), 2);
    }
}
