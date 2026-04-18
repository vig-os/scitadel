// MCP tool definitions for scitadel.
//
// This module defines the tool handlers that will be exposed via rmcp.
// Each tool: validate input, call service, format response.
//
// Note: rmcp integration requires the rmcp crate's macro system.
// This is a structural placeholder — the actual rmcp server setup
// depends on the rmcp API which may change. The tool logic is complete.

use scitadel_core::config::load_config;
use scitadel_core::models::{Assessment, Paper, ResearchQuestion, SearchTerm};
use scitadel_core::ports::{
    AssessmentRepository, PaperRepository, QuestionRepository, SearchRepository,
};
use scitadel_db::sqlite::Database;
use scitadel_scoring::{SCORING_SYSTEM_PROMPT, build_user_prompt};

fn open_db() -> Result<Database, String> {
    let config = load_config();
    let db = Database::open(&config.db_path).map_err(|e| e.to_string())?;
    db.migrate().map_err(|e| e.to_string())?;
    Ok(db)
}

pub async fn search_tool(
    query: String,
    sources: String,
    max_results: usize,
    question_id: Option<String>,
) -> Result<String, String> {
    let config = load_config();
    let source_list: Vec<String> = sources.split(',').map(|s| s.trim().to_string()).collect();

    let mut query = if query.is_empty() { None } else { Some(query) };
    let mut parameters = serde_json::Map::new();

    if let Some(ref qid) = question_id {
        let db = open_db()?;
        let (_, _, q_repo, _, _) = db.repositories();
        let question = q_repo
            .get_question(qid)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Question '{qid}' not found."))?;

        parameters.insert(
            "question_id".into(),
            serde_json::Value::String(question.id.as_str().to_string()),
        );

        if query.is_none() {
            let terms = q_repo
                .get_terms(question.id.as_str())
                .map_err(|e| e.to_string())?;
            if terms.is_empty() {
                return Err(format!(
                    "No search terms linked to question '{}'.",
                    question.id.short()
                ));
            }
            query = Some(
                terms
                    .iter()
                    .filter(|t| !t.query_string.is_empty())
                    .map(|t| t.query_string.as_str())
                    .collect::<Vec<_>>()
                    .join(" OR "),
            );
        }
    }

    let query = query.ok_or("Provide a query or question_id with linked search terms.")?;

    let adapters = scitadel_adapters::build_adapters_full(
        &source_list,
        &config.pubmed.api_key,
        &config.openalex.api_key,
        &config.patentsview.api_key,
        &config.lens.api_key,
        &config.epo.consumer_key,
        &config.epo.consumer_secret,
    )
    .map_err(|e| e.to_string())?;

    let (mut search_record, candidates) =
        scitadel_core::services::orchestrator::run_search(&query, &adapters, max_results, 3).await;

    let (papers, mut search_results) =
        scitadel_core::services::dedup::deduplicate(&candidates, 0.85);
    search_record.total_papers = papers.len() as i32;

    let db = open_db()?;
    let (paper_repo, search_repo, _, _, _) = db.repositories();

    let id_remap = paper_repo.save_many(&papers).map_err(|e| e.to_string())?;
    search_repo
        .save(&search_record)
        .map_err(|e| e.to_string())?;

    for sr in &mut search_results {
        sr.search_id = search_record.id.clone();
        if let Some(new_id) = id_remap.get(&sr.paper_id) {
            sr.paper_id = new_id.clone();
        }
    }
    search_repo
        .save_results(&search_results)
        .map_err(|e| e.to_string())?;

    let outcomes: Vec<String> = search_record
        .source_outcomes
        .iter()
        .map(|o| {
            format!(
                "  {}: {} results ({}, {:.0}ms)",
                o.source, o.result_count, o.status, o.latency_ms
            )
        })
        .collect();

    Ok(format!(
        "Search ID: {}\nQuery: {query}\nSources: {}\nTotal candidates: {}\nUnique papers after dedup: {}\n{}",
        search_record.id,
        source_list.join(", "),
        search_record.total_candidates,
        papers.len(),
        outcomes.join("\n")
    ))
}

pub fn list_searches_tool(limit: i64) -> Result<String, String> {
    let db = open_db()?;
    let (_, search_repo, _, _, _) = db.repositories();
    let searches = search_repo
        .list_searches(limit)
        .map_err(|e| e.to_string())?;

    if searches.is_empty() {
        return Ok("No search history found.".into());
    }

    let lines: Vec<String> = searches
        .iter()
        .map(|s| {
            let success = s
                .source_outcomes
                .iter()
                .filter(|o| o.status == scitadel_core::models::SourceStatus::Success)
                .count();
            format!(
                "{}  {}  \"{}\"  {} papers  {}/{} sources ok",
                s.id.short(),
                s.created_at.format("%Y-%m-%d %H:%M"),
                s.query,
                s.total_papers,
                success,
                s.source_outcomes.len()
            )
        })
        .collect();

    Ok(lines.join("\n"))
}

pub fn get_papers_tool(search_id: &str) -> Result<String, String> {
    let db = open_db()?;
    let (paper_repo, search_repo, _, _, _) = db.repositories();

    let search = search_repo
        .get(search_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Search '{search_id}' not found."))?;

    let results = search_repo
        .get_results(search.id.as_str())
        .map_err(|e| e.to_string())?;

    let paper_ids: std::collections::HashSet<&str> =
        results.iter().map(|r| r.paper_id.as_str()).collect();
    let papers: Vec<Paper> = paper_ids
        .iter()
        .filter_map(|id| paper_repo.get(id).ok().flatten())
        .collect();

    let mut out = vec![format!(
        "Search: {} — \"{}\" — {} papers\n",
        search.id.short(),
        search.query,
        papers.len()
    )];

    for (i, p) in papers.iter().enumerate() {
        let authors = p
            .authors
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let authors_suffix = if p.authors.len() > 3 {
            format!(" et al. ({} total)", p.authors.len())
        } else {
            String::new()
        };
        let abstract_preview = if p.r#abstract.len() > 300 {
            format!("{}...", &p.r#abstract[..300])
        } else {
            p.r#abstract.clone()
        };

        out.push(format!(
            "[{}] {}\n    Authors: {}{}\n    Year: {}  Journal: {}\n    DOI: {}  ID: {}\n    Abstract: {}\n",
            i + 1,
            p.title,
            authors,
            authors_suffix,
            p.year.map_or_else(|| "N/A".into(), |y| y.to_string()),
            p.journal.as_deref().unwrap_or("N/A"),
            p.doi.as_deref().unwrap_or("N/A"),
            p.id.short(),
            abstract_preview
        ));
    }

    Ok(out.join("\n"))
}

pub fn get_paper_tool(paper_id: &str) -> Result<String, String> {
    let db = open_db()?;
    let (paper_repo, _, _, _, _) = db.repositories();

    let paper = paper_repo
        .get(paper_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Paper '{paper_id}' not found."))?;

    serde_json::to_string_pretty(&paper).map_err(|e| e.to_string())
}

pub fn export_search_tool(search_id: &str, format: &str) -> Result<String, String> {
    let db = open_db()?;
    let (paper_repo, search_repo, _, _, _) = db.repositories();

    let search = search_repo
        .get(search_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Search '{search_id}' not found."))?;

    let results = search_repo
        .get_results(search.id.as_str())
        .map_err(|e| e.to_string())?;
    let paper_ids: std::collections::HashSet<&str> =
        results.iter().map(|r| r.paper_id.as_str()).collect();
    let papers: Vec<Paper> = paper_ids
        .iter()
        .filter_map(|id| paper_repo.get(id).ok().flatten())
        .collect();

    match format {
        "csv" => Ok(scitadel_export::export_csv(&papers)),
        "bibtex" => Ok(scitadel_export::export_bibtex(&papers)),
        // Default to JSON for unknown formats
        _ => Ok(scitadel_export::export_json(&papers, 2)),
    }
}

pub fn create_question_tool(text: &str, description: &str) -> Result<String, String> {
    let db = open_db()?;
    let (_, _, q_repo, _, _) = db.repositories();

    let mut question = ResearchQuestion::new(text);
    question.description = description.to_string();
    q_repo.save_question(&question).map_err(|e| e.to_string())?;

    Ok(format!(
        "Question created: {}\nText: {text}",
        question.id.short()
    ))
}

pub fn list_questions_tool() -> Result<String, String> {
    let db = open_db()?;
    let (_, _, q_repo, _, _) = db.repositories();
    let questions = q_repo.list_questions().map_err(|e| e.to_string())?;

    if questions.is_empty() {
        return Ok("No research questions found.".into());
    }

    let lines: Vec<String> = questions
        .iter()
        .map(|q| {
            format!(
                "{}  {}  \"{}\"",
                q.id.short(),
                q.created_at.format("%Y-%m-%d %H:%M"),
                q.text
            )
        })
        .collect();

    Ok(lines.join("\n"))
}

pub fn add_search_terms_tool(
    question_id: &str,
    terms: &[String],
    query_string: &str,
) -> Result<String, String> {
    let db = open_db()?;
    let (_, _, q_repo, _, _) = db.repositories();

    let question = q_repo
        .get_question(question_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Question '{question_id}' not found."))?;

    let query_str = if query_string.is_empty() {
        terms.join(" ")
    } else {
        query_string.to_string()
    };

    let mut term = SearchTerm::new(question.id.clone());
    term.terms = terms.to_vec();
    term.query_string = query_str;
    q_repo.save_term(&term).map_err(|e| e.to_string())?;

    Ok(format!(
        "Search terms added to question {}: {:?}",
        question.id.short(),
        terms
    ))
}

pub fn assess_paper_tool(
    paper_id: &str,
    question_id: &str,
    score: f64,
    reasoning: &str,
    assessor: &str,
    model: Option<&str>,
) -> Result<String, String> {
    let db = open_db()?;
    let (paper_repo, _, q_repo, a_repo, _) = db.repositories();

    let paper = paper_repo
        .get(paper_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Paper '{paper_id}' not found."))?;

    let question = q_repo
        .get_question(question_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Question '{question_id}' not found."))?;

    let mut assessment = Assessment::new(paper.id.clone(), question.id.clone(), score);
    assessment.reasoning = reasoning.to_string();
    assessment.assessor = assessor.to_string();
    assessment.model = model.map(String::from);

    a_repo.save(&assessment).map_err(|e| e.to_string())?;

    Ok(format!(
        "Assessment saved: {}\nPaper: {}\nQuestion: {}\nScore: {score:.2}\nReasoning: {}",
        assessment.id.short(),
        &paper.title[..paper.title.len().min(60)],
        &question.text[..question.text.len().min(60)],
        &reasoning[..reasoning.len().min(200)]
    ))
}

pub fn get_assessments_tool(
    paper_id: Option<&str>,
    question_id: Option<&str>,
) -> Result<String, String> {
    let db = open_db()?;
    let (paper_repo, _, _, a_repo, _) = db.repositories();

    let assessments = if let Some(pid) = paper_id {
        a_repo
            .get_for_paper(pid, question_id)
            .map_err(|e| e.to_string())?
    } else if let Some(qid) = question_id {
        a_repo.get_for_question(qid).map_err(|e| e.to_string())?
    } else {
        return Err("Provide at least one of paper_id or question_id.".into());
    };

    if assessments.is_empty() {
        return Ok("No assessments found.".into());
    }

    let lines: Vec<String> = assessments
        .iter()
        .map(|a| {
            let title = paper_repo
                .get(a.paper_id.as_str())
                .ok()
                .flatten()
                .map_or_else(
                    || "Unknown".into(),
                    |p| p.title[..p.title.len().min(50)].to_string(),
                );
            format!(
                "Score: {:.2}  Paper: {}  Assessor: {}  {}\n  Reasoning: {}",
                a.score,
                title,
                a.assessor,
                a.created_at.format("%Y-%m-%d %H:%M"),
                &a.reasoning[..a.reasoning.len().min(200)]
            )
        })
        .collect();

    Ok(lines.join("\n\n"))
}

/// Prepare an assessment rubric and paper data for the host LLM to evaluate inline.
///
/// Returns the scoring rubric (system prompt) and filled user prompt so the host
/// LLM can evaluate the paper directly, then call `save_assessment` with the result.
pub fn prepare_assessment_tool(paper_id: &str, question_id: &str) -> Result<String, String> {
    let db = open_db()?;
    let (paper_repo, _, q_repo, _, _) = db.repositories();

    let paper = paper_repo
        .get(paper_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Paper '{paper_id}' not found."))?;

    let question = q_repo
        .get_question(question_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Question '{question_id}' not found."))?;

    let user_prompt = build_user_prompt(&paper, &question);

    Ok(format!(
        "=== SCORING RUBRIC ===\n\
         {SCORING_SYSTEM_PROMPT}\n\n\
         === PAPER TO EVALUATE ===\n\
         {user_prompt}\n\n\
         === INSTRUCTIONS ===\n\
         Evaluate this paper using the rubric above. Then call `save_assessment` with:\n\
         - paper_id: \"{paper_id}\"\n\
         - question_id: \"{question_id}\"\n\
         - score: <your 0.0-1.0 score>\n\
         - reasoning: <your 1-3 sentence reasoning>"
    ))
}

/// Save an MCP-native assessment (scored by the host LLM, not a subprocess).
///
/// Validates that score is in 0.0-1.0 range and persists the assessment with
/// assessor set to "mcp-native".
pub fn save_assessment_tool(
    paper_id: &str,
    question_id: &str,
    score: f64,
    reasoning: &str,
) -> Result<String, String> {
    if !(0.0..=1.0).contains(&score) {
        return Err(format!("Score must be between 0.0 and 1.0, got {score:.2}"));
    }

    let db = open_db()?;
    let (paper_repo, _, q_repo, a_repo, _) = db.repositories();

    let paper = paper_repo
        .get(paper_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Paper '{paper_id}' not found."))?;

    let question = q_repo
        .get_question(question_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Question '{question_id}' not found."))?;

    let mut assessment = Assessment::new(paper.id.clone(), question.id.clone(), score);
    assessment.reasoning = reasoning.to_string();
    assessment.assessor = "mcp-native".to_string();
    assessment.model = None;

    a_repo.save(&assessment).map_err(|e| e.to_string())?;

    Ok(format!(
        "Assessment saved: {}\nPaper: {}\nQuestion: {}\nScore: {score:.2}\nAssessor: mcp-native\nReasoning: {}",
        assessment.id.short(),
        &paper.title[..paper.title.len().min(60)],
        &question.text[..question.text.len().min(60)],
        &reasoning[..reasoning.len().min(200)]
    ))
}

/// Download a paper. If `paper_id` is provided, uses the full multi-source chain
/// (arxiv → openalex → doi/Unpaywall → publisher) against the stored Paper record.
/// Otherwise falls back to DOI-only.
pub async fn download_paper_tool(
    paper_id: Option<&str>,
    doi: Option<&str>,
    output_dir: Option<&str>,
) -> Result<String, String> {
    let config = load_config();
    let out_dir = output_dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.papers_dir());

    let downloader =
        scitadel_adapters::download::PaperDownloader::new(config.openalex.api_key.clone(), 60.0);

    let result = if let Some(pid) = paper_id {
        let db = open_db()?;
        let (paper_repo, _, _, _, _) = db.repositories();
        let paper = paper_repo
            .get(pid)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("paper not found: {pid}"))?;
        downloader
            .download_paper(&paper, &out_dir)
            .await
            .map_err(|e| e.to_string())?
    } else if let Some(d) = doi {
        downloader
            .download(d, &out_dir)
            .await
            .map_err(|e| e.to_string())?
    } else {
        return Err("need either paper_id or doi".into());
    };

    Ok(format!(
        "Downloaded paper: {}\nFormat: {}\nSource: {}\nAccess: {}\nSize: {} bytes\nPath: {}",
        result.doi,
        result.format,
        result.source,
        result.access,
        result.bytes,
        result.path.display()
    ))
}

/// Extract text from a paper's downloaded file (PDF or HTML).
///
/// Looks up the paper in the DB, locates its cached file under `papers_dir()`,
/// and returns the extracted text. Truncated to `max_chars` (default 20_000) to
/// keep responses manageable for the host LLM.
pub async fn read_paper_tool(paper_id: &str, max_chars: Option<usize>) -> Result<String, String> {
    let config = load_config();
    let db = open_db()?;
    let (paper_repo, _, _, _, _) = db.repositories();
    let paper = paper_repo
        .get(paper_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("paper not found: {paper_id}"))?;

    let path = scitadel_adapters::download::find_cached_file(&paper, &config.papers_dir())
        .ok_or_else(|| "paper not downloaded yet. Call download_paper first.".to_string())?;

    let text = match path.extension().and_then(|e| e.to_str()) {
        Some("pdf") => {
            let path_clone = path.clone();
            tokio::task::spawn_blocking(move || pdf_extract::extract_text(&path_clone))
                .await
                .map_err(|e| format!("pdf extract task failed: {e}"))?
                .map_err(|e| format!("pdf extract failed: {e}"))?
        }
        Some("html") => {
            let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
            let html = String::from_utf8_lossy(&bytes);
            html_to_text(&html)
        }
        other => return Err(format!("unsupported file type: {other:?}")),
    };

    let limit = max_chars.unwrap_or(20_000);
    let truncated = if text.chars().count() > limit {
        let head: String = text.chars().take(limit).collect();
        format!(
            "{head}\n\n[... truncated, {} of {} chars shown ...]",
            limit,
            text.chars().count()
        )
    } else {
        text
    };

    Ok(format!(
        "Paper: {}\nPath: {}\n\n{}",
        paper.title,
        path.display(),
        truncated
    ))
}

fn html_to_text(html: &str) -> String {
    let doc = scraper::Html::parse_document(html);
    let mut text = String::new();
    for node in doc.root_element().descendants() {
        let Some(t) = node.value().as_text() else {
            continue;
        };
        let skip_subtree = node.ancestors().any(|a| {
            a.value()
                .as_element()
                .is_some_and(|el| matches!(el.name(), "script" | "style" | "noscript"))
        });
        if skip_subtree {
            continue;
        }
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            text.push_str(trimmed);
            text.push(' ');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::html_to_text;

    #[test]
    fn strips_tags_and_script() {
        let html = "<html><body><p>Hello <b>world</b>.</p><script>var x=1;</script></body></html>";
        let out = html_to_text(html);
        assert!(out.contains("Hello"));
        assert!(out.contains("world"));
        assert!(!out.contains("var x"));
    }
}

/// Prepare batch assessments for all papers in a search.
///
/// Returns the rubric once, then a summary of each paper so the host LLM can
/// evaluate them all and call `save_assessment` for each.
pub fn prepare_batch_assessments_tool(
    search_id: &str,
    question_id: &str,
) -> Result<String, String> {
    let db = open_db()?;
    let (paper_repo, search_repo, q_repo, _, _) = db.repositories();

    let search = search_repo
        .get(search_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Search '{search_id}' not found."))?;

    let question = q_repo
        .get_question(question_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Question '{question_id}' not found."))?;

    let results = search_repo
        .get_results(search.id.as_str())
        .map_err(|e| e.to_string())?;

    let paper_ids: std::collections::HashSet<&str> =
        results.iter().map(|r| r.paper_id.as_str()).collect();
    let papers: Vec<Paper> = paper_ids
        .iter()
        .filter_map(|id| paper_repo.get(id).ok().flatten())
        .collect();

    if papers.is_empty() {
        return Ok(format!(
            "No papers found for search '{}'.",
            search.id.short()
        ));
    }

    let mut out = vec![format!(
        "=== SCORING RUBRIC ===\n\
         {SCORING_SYSTEM_PROMPT}\n\n\
         === RESEARCH QUESTION ===\n\
         {}\n\
         {}\n\n\
         === PAPERS TO EVALUATE ({} total) ===\n",
        question.text,
        if question.description.is_empty() {
            String::new()
        } else {
            format!("Context: {}", question.description)
        },
        papers.len()
    )];

    for (i, p) in papers.iter().enumerate() {
        let authors = p
            .authors
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let authors_suffix = if p.authors.len() > 3 {
            format!(" et al. ({} total)", p.authors.len())
        } else {
            String::new()
        };
        let abstract_preview = if p.r#abstract.len() > 300 {
            format!("{}...", &p.r#abstract[..300])
        } else {
            p.r#abstract.clone()
        };

        out.push(format!(
            "[{}] {}\n\
             \x20   Authors: {}{}\n\
             \x20   Year: {}  Journal: {}\n\
             \x20   Paper ID: {}\n\
             \x20   Abstract: {}\n",
            i + 1,
            p.title,
            authors,
            authors_suffix,
            p.year.map_or_else(|| "N/A".into(), |y| y.to_string()),
            p.journal.as_deref().unwrap_or("N/A"),
            p.id.as_str(),
            abstract_preview
        ));
    }

    out.push(format!(
        "=== INSTRUCTIONS ===\n\
         Evaluate each paper above against the research question using the rubric.\n\
         For each paper, call `save_assessment` with:\n\
         - paper_id: <the paper's ID>\n\
         - question_id: \"{question_id}\"\n\
         - score: <your 0.0-1.0 score>\n\
         - reasoning: <your 1-3 sentence reasoning>"
    ));

    Ok(out.join("\n"))
}
