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

    let outcome_lines: Vec<String> = search_record
        .source_outcomes
        .iter()
        .map(|o| {
            format!(
                "  {}: {} results ({}, {:.0}ms)",
                o.source, o.result_count, o.status, o.latency_ms
            )
        })
        .collect();

    let summary = format!(
        "Search ID: {}\nQuery: {query}\nSources: {}\nTotal candidates: {}\nUnique papers after dedup: {}\n{}",
        search_record.id,
        source_list.join(", "),
        search_record.total_candidates,
        papers.len(),
        outcome_lines.join("\n")
    );

    // Structured payload: agents introspect per-source status + counts
    // without parsing the summary string. `summary` field kept so
    // existing string-consuming clients keep working.
    let payload = serde_json::json!({
        "search_id": search_record.id.as_str(),
        "query": search_record.query,
        "sources": search_record.source_outcomes,
        "total_candidates": search_record.total_candidates,
        "total_unique_papers": papers.len(),
        "summary": summary,
    });

    serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())
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
        let (abstract_preview, _) = truncate_abstract(&p.r#abstract, 300);

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
    let out_dir = output_dir.map_or_else(|| config.papers_dir(), std::path::PathBuf::from);

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
        let (abstract_preview, _) = truncate_abstract(&p.r#abstract, 300);

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

// ---------- Annotations (#49 iter 4 + 5) ----------

/// Record that `reader` has seen the current state of one or more
/// annotations. Idempotent: repeat calls just bump the `seen_at`.
pub fn mark_seen_tool(annotation_ids: Vec<String>, reader: &str) -> Result<String, String> {
    if reader.trim().is_empty() {
        return Err("reader is required".into());
    }
    let refs: Vec<&str> = annotation_ids.iter().map(String::as_str).collect();
    let db = open_db()?;
    let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);
    repo.mark_seen(&refs, reader).map_err(|e| e.to_string())?;
    Ok(format!(
        "Marked {} annotation(s) seen for '{reader}'.",
        refs.len()
    ))
}

/// Mark a whole thread (root + all replies) as seen by `reader`.
pub fn mark_thread_seen_tool(root_id: &str, reader: &str) -> Result<String, String> {
    if reader.trim().is_empty() {
        return Err("reader is required".into());
    }
    let db = open_db()?;
    let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);
    repo.mark_thread_seen(root_id, reader)
        .map_err(|e| e.to_string())?;
    Ok(format!("Thread {root_id} marked seen for '{reader}'."))
}

/// List annotations `reader` hasn't seen since the last modification.
/// Optional `paper_id` scopes the query. Returns the same JSON shape as
/// `list_annotations` for easy consumption.
pub fn list_unread_tool(reader: &str, paper_id: Option<&str>) -> Result<String, String> {
    if reader.trim().is_empty() {
        return Err("reader is required".into());
    }
    let db = open_db()?;
    let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);
    let rows = repo
        .list_unread(reader, paper_id)
        .map_err(|e| e.to_string())?;

    let entries: Vec<serde_json::Value> = rows
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id.as_str(),
                "parent_id": a.parent_id.as_ref().map(|p| p.as_str()),
                "paper_id": a.paper_id.as_str(),
                "question_id": a.question_id.as_ref().map(|q| q.as_str()),
                "anchor": {
                    "char_range": a.anchor.char_range,
                    "quote": a.anchor.quote,
                    "status": a.anchor.status.as_str(),
                },
                "note": a.note,
                "author": a.author,
                "updated_at": a.updated_at.to_rfc3339(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())
}

// ---------- Annotations (#49 iter 4) ----------

/// Create a root-level annotation anchored to a passage in a paper.
/// Replies use `reply_annotation_tool` since they inherit the anchor.
#[allow(clippy::too_many_arguments)]
pub fn create_annotation_tool(
    paper_id: &str,
    quote: &str,
    note: &str,
    author: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
    question_id: Option<&str>,
    color: Option<&str>,
    tags: Option<Vec<String>>,
) -> Result<String, String> {
    if author.trim().is_empty() {
        return Err("author is required (pass an identity string)".into());
    }
    let db = open_db()?;
    let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);

    let anchor = scitadel_core::models::Anchor {
        quote: Some(quote.to_string()),
        prefix: prefix.map(str::to_string),
        suffix: suffix.map(str::to_string),
        status: scitadel_core::models::AnchorStatus::Ok,
        ..Default::default()
    };

    let mut ann = scitadel_core::models::Annotation::new_root(
        scitadel_core::models::PaperId::from(paper_id),
        author.to_string(),
        note.to_string(),
        anchor,
    );
    if let Some(qid) = question_id {
        ann.question_id = Some(scitadel_core::models::QuestionId::from(qid));
    }
    if let Some(c) = color {
        ann.color = Some(c.to_string());
    }
    if let Some(t) = tags {
        ann.tags = t;
    }

    repo.create(&ann).map_err(|e| e.to_string())?;
    Ok(ann.id.as_str().to_string())
}

/// Add a reply to an existing annotation. Inherits paper_id and
/// question_id from the parent.
pub fn reply_annotation_tool(parent_id: &str, note: &str, author: &str) -> Result<String, String> {
    if author.trim().is_empty() {
        return Err("author is required".into());
    }
    let db = open_db()?;
    let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);
    let parent = repo
        .get(parent_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Annotation '{parent_id}' not found."))?;
    let reply =
        scitadel_core::models::Annotation::new_reply(&parent, author.to_string(), note.to_string());
    repo.create(&reply).map_err(|e| e.to_string())?;
    Ok(reply.id.as_str().to_string())
}

/// Update mutable fields on an existing annotation.
pub fn update_annotation_tool(
    id: &str,
    note: Option<&str>,
    color: Option<&str>,
    tags: Option<Vec<String>>,
) -> Result<String, String> {
    let db = open_db()?;
    let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);
    let existing = repo
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Annotation '{id}' not found."))?;
    let new_note = note.unwrap_or(&existing.note);
    let new_color = color.or(existing.color.as_deref());
    let new_tags = tags.unwrap_or(existing.tags);
    repo.update_note(id, new_note, new_color, &new_tags)
        .map_err(|e| e.to_string())?;
    Ok(format!("Annotation {id} updated."))
}

/// Soft-delete an annotation. Keeps the row so threads are preserved;
/// `list_annotations` hides it.
pub fn delete_annotation_tool(id: &str) -> Result<String, String> {
    let db = open_db()?;
    let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);
    repo.soft_delete(id).map_err(|e| e.to_string())?;
    Ok(format!("Annotation {id} deleted (soft)."))
}

/// List annotations filtered by paper / question / author. Returns a
/// JSON array with id, parent_id, anchor, note, tags, author,
/// timestamps, and anchor_status.
pub fn list_annotations_tool(
    paper_id: Option<&str>,
    author: Option<&str>,
) -> Result<String, String> {
    let db = open_db()?;
    let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db);
    let rows = match paper_id {
        Some(pid) => repo.list_by_paper(pid).map_err(|e| e.to_string())?,
        None => return Err("paper_id is required for now (cross-paper lists come later)".into()),
    };
    let filtered: Vec<_> = rows
        .into_iter()
        .filter(|a| author.is_none_or(|want| a.author == want))
        .collect();

    let entries: Vec<serde_json::Value> = filtered
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id.as_str(),
                "parent_id": a.parent_id.as_ref().map(|p| p.as_str()),
                "paper_id": a.paper_id.as_str(),
                "question_id": a.question_id.as_ref().map(|q| q.as_str()),
                "anchor": {
                    "char_range": a.anchor.char_range,
                    "quote": a.anchor.quote,
                    "prefix": a.anchor.prefix,
                    "suffix": a.anchor.suffix,
                    "status": a.anchor.status.as_str(),
                },
                "note": a.note,
                "color": a.color,
                "tags": a.tags,
                "author": a.author,
                "created_at": a.created_at.to_rfc3339(),
                "updated_at": a.updated_at.to_rfc3339(),
            })
        })
        .collect();

    serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())
}

/// Return the paper text + every live annotation anchored to it as a
/// single structured JSON document, so an agent can reason over offsets
/// without re-deriving the mapping from `get_paper` + `list_annotations`.
///
/// Soft-deleted annotations are excluded. Replies are flat with
/// `parent_id` and `root_id` so threads can be reconstructed without
/// extra round-trips.
pub fn get_annotated_paper_tool(paper_id: &str) -> Result<String, String> {
    let db = open_db()?;
    build_annotated_paper(&db, paper_id)
}

fn build_annotated_paper(db: &Database, paper_id: &str) -> Result<String, String> {
    use std::collections::HashMap;

    let (paper_repo, _, _, _, _) = db.repositories();
    let paper = paper_repo
        .get(paper_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Paper '{paper_id}' not found."))?;

    let ann_repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(db.clone());
    let annotations = ann_repo
        .list_by_paper(paper_id)
        .map_err(|e| e.to_string())?;

    let by_id: HashMap<&str, &scitadel_core::models::Annotation> =
        annotations.iter().map(|a| (a.id.as_str(), a)).collect();
    let root_of = |start: &scitadel_core::models::Annotation| -> String {
        let mut cur = start;
        // Bounded chase to avoid pathological loops if data is malformed.
        for _ in 0..64 {
            match &cur.parent_id {
                None => return cur.id.as_str().to_string(),
                Some(pid) => match by_id.get(pid.as_str()) {
                    Some(parent) => cur = parent,
                    None => return cur.id.as_str().to_string(),
                },
            }
        }
        cur.id.as_str().to_string()
    };

    let source_version = annotations
        .iter()
        .find_map(|a| a.anchor.source_version.clone());

    let entries: Vec<serde_json::Value> = annotations
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id.as_str(),
                "parent_id": a.parent_id.as_ref().map(|p| p.as_str()),
                "root_id": root_of(a),
                "paper_id": a.paper_id.as_str(),
                "question_id": a.question_id.as_ref().map(|q| q.as_str()),
                "anchor": {
                    "char_range": a.anchor.char_range,
                    "quote": a.anchor.quote,
                    "prefix": a.anchor.prefix,
                    "suffix": a.anchor.suffix,
                    "sentence_id": a.anchor.sentence_id,
                    "source_version": a.anchor.source_version,
                    "status": a.anchor.status.as_str(),
                },
                "note": a.note,
                "color": a.color,
                "tags": a.tags,
                "author": a.author,
                "created_at": a.created_at.to_rfc3339(),
                "updated_at": a.updated_at.to_rfc3339(),
            })
        })
        .collect();

    let response = serde_json::json!({
        "paper": {
            "id": paper.id.as_str(),
            "title": paper.title,
            "abstract": paper.r#abstract,
            "full_text": paper.full_text,
        },
        "annotations": entries,
        "source_version": source_version,
    });

    serde_json::to_string_pretty(&response).map_err(|e| e.to_string())
}

/// Full-text search over stored search queries. Returns a JSON array of
/// past searches matching `query` (lower `rank` = more relevant per
/// BM25). Agents should call this before running a fresh search to
/// avoid re-doing work that's already in the DB.
pub fn find_similar_searches_tool(query: &str, limit: Option<i64>) -> Result<String, String> {
    let limit = limit.unwrap_or(10).max(1);
    let db = open_db()?;
    let (_, search_repo, _, _, _) = db.repositories();
    let hits = search_repo
        .find_similar(query, limit)
        .map_err(|e| e.to_string())?;

    let entries: Vec<serde_json::Value> = hits
        .into_iter()
        .map(|(s, rank)| {
            serde_json::json!({
                "search_id": s.id.as_str(),
                "query": s.query,
                "total_papers": s.total_papers,
                "created_at": s.created_at.to_rfc3339(),
                "rank": rank,
            })
        })
        .collect();

    serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())
}

/// Return the scitadel assessment rubric (scoring criteria, score scale,
/// response format) so an agent can fetch it once and cache, rather than
/// re-fetching via `prepare_assessment` for every paper it evaluates.
///
/// Today the rubric is a static prompt shared across all questions;
/// if per-question rubrics ever land, this function becomes the
/// customization point.
pub fn get_rubric_tool() -> Result<String, String> {
    Ok(scitadel_scoring::SCORING_SYSTEM_PROMPT.to_string())
}

/// Summarize every paper in a search as JSON — title, abstract, access
/// status, identifiers — in a single call. Saves round-trips vs. the
/// agent iterating `get_paper` per result.
///
/// `max_papers` caps the output (default 50). Abstracts are truncated
/// at `abstract_char_limit` (default 500) with an ellipsis if cut.
pub fn summarize_search_tool(
    search_id: &str,
    max_papers: Option<usize>,
    abstract_char_limit: Option<usize>,
) -> Result<String, String> {
    let max_papers = max_papers.unwrap_or(50);
    let abstract_char_limit = abstract_char_limit.unwrap_or(500);

    let db = open_db()?;
    let (paper_repo, search_repo, _, _, _) = db.repositories();

    let search = search_repo
        .get(search_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Search '{search_id}' not found."))?;

    let results = search_repo
        .get_results(search.id.as_str())
        .map_err(|e| e.to_string())?;

    // Unique paper IDs in the order they were returned by the adapters.
    let mut seen = std::collections::HashSet::new();
    let mut ordered_ids: Vec<&str> = Vec::new();
    for r in &results {
        if seen.insert(r.paper_id.as_str()) {
            ordered_ids.push(r.paper_id.as_str());
        }
    }
    ordered_ids.truncate(max_papers);

    let papers: Vec<Paper> = ordered_ids
        .iter()
        .filter_map(|id| paper_repo.get(id).ok().flatten())
        .collect();

    let summaries: Vec<serde_json::Value> = papers
        .iter()
        .map(|p| {
            let (abstract_text, truncated) = truncate_abstract(&p.r#abstract, abstract_char_limit);
            serde_json::json!({
                "paper_id": p.id.as_str(),
                "title": p.title,
                "authors": p.authors,
                "year": p.year,
                "journal": p.journal,
                "doi": p.doi,
                "arxiv_id": p.arxiv_id,
                "openalex_id": p.openalex_id,
                "abstract": abstract_text,
                "abstract_truncated": truncated,
            })
        })
        .collect();

    let payload = serde_json::json!({
        "search_id": search.id.as_str(),
        "query": search.query,
        "total_papers": search.total_papers,
        "returned": summaries.len(),
        "papers": summaries,
    });

    serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())
}

/// Truncate an abstract at a char boundary. Returns `(text, was_truncated)`
/// so the caller can signal truncation in the JSON payload. Appends an
/// ellipsis only when actually cut.
fn truncate_abstract(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("...");
    (out, true)
}

/// Static descriptor for one source adapter — used by `list_sources_tool`.
struct SourceInfo {
    name: &'static str,
    description: &'static str,
    /// Which keychain / env fields need to be populated before this
    /// adapter can actually make requests. Empty = no credentials needed.
    credential_fields: &'static [&'static str],
    rate_limit_hint: &'static str,
}

const SOURCE_REGISTRY: &[SourceInfo] = &[
    SourceInfo {
        name: "pubmed",
        description: "US NLM biomedical/life-sciences literature via the NCBI E-utilities API.",
        credential_fields: &["api_key"],
        rate_limit_hint: "3 req/s without key, 10 req/s with key.",
    },
    SourceInfo {
        name: "arxiv",
        description: "Preprint server for physics, CS, math, and adjacent fields.",
        credential_fields: &[],
        rate_limit_hint: "1 req every 3s per the arXiv API terms.",
    },
    SourceInfo {
        name: "openalex",
        description: "Open scholarly-works graph covering most disciplines. The polite-pool email gets 10 req/s; without it you share the global 10 req/s pool.",
        // Stored under `openalex.api_key` in config for historical reasons;
        // users authenticate by putting their contact email there, not a real key.
        credential_fields: &["polite_pool_email"],
        rate_limit_hint: "10 req/s in the polite pool (with email), otherwise shared.",
    },
    SourceInfo {
        name: "inspire",
        description: "INSPIRE-HEP: high-energy physics literature and preprints.",
        credential_fields: &[],
        rate_limit_hint: "15 req/5s per their API guidelines.",
    },
    SourceInfo {
        name: "patentsview",
        description: "USPTO PatentsView API — US patent metadata.",
        credential_fields: &["api_key"],
        rate_limit_hint: "45 req/min (documented).",
    },
    SourceInfo {
        name: "lens",
        description: "Lens.org scholarly + patent metadata.",
        credential_fields: &["api_token"],
        rate_limit_hint: "Varies by plan; monthly quota enforced.",
    },
    SourceInfo {
        name: "epo",
        description: "European Patent Office OPS API (consumer_key + consumer_secret).",
        credential_fields: &["consumer_key", "consumer_secret"],
        rate_limit_hint: "10 req/min per OPS free tier.",
    },
];

/// Return a JSON array describing every source scitadel knows about, with
/// each one's credential requirements and whether they're configured in
/// the current environment. Read-only; safe to call freely.
pub fn list_sources_tool() -> Result<String, String> {
    let config = load_config();

    let entries: Vec<serde_json::Value> = SOURCE_REGISTRY
        .iter()
        .map(|src| {
            let configured = is_source_configured(src, &config);
            serde_json::json!({
                "name": src.name,
                "description": src.description,
                "requires_credentials": !src.credential_fields.is_empty(),
                "credential_fields": src.credential_fields,
                "configured": configured,
                "rate_limit_hint": src.rate_limit_hint,
            })
        })
        .collect();

    serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())
}

fn is_source_configured(src: &SourceInfo, config: &scitadel_core::config::Config) -> bool {
    match src.name {
        // Sources with no credentials are always considered configured.
        "arxiv" | "inspire" => true,
        "pubmed" => !config.pubmed.api_key.is_empty(),
        // OpenAlex stores the polite-pool email under `openalex.api_key`
        // (historical naming); an empty string means polite pool is off
        // but the adapter still works, so we report configured only when
        // the email is actually present.
        "openalex" => !config.openalex.api_key.is_empty(),
        "patentsview" => !config.patentsview.api_key.is_empty(),
        "lens" => !config.lens.api_key.is_empty(),
        "epo" => !config.epo.consumer_key.is_empty() && !config.epo.consumer_secret.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SOURCE_REGISTRY, build_annotated_paper, html_to_text, is_source_configured,
        truncate_abstract,
    };
    use scitadel_core::config::Config;
    use scitadel_core::models::{Anchor, Annotation, Paper, PaperId};
    use scitadel_core::ports::PaperRepository;
    use scitadel_db::sqlite::{Database, SqliteAnnotationRepository};

    fn fresh_db() -> Database {
        let db = Database::open_in_memory().expect("open in-memory db");
        db.migrate().expect("migrate");
        db
    }

    fn save_paper(db: &Database, id: &str, title: &str, full_text: Option<&str>) -> PaperId {
        let (paper_repo, _, _, _, _) = db.repositories();
        let mut p = Paper::new(title);
        p.id = PaperId::from(id);
        p.r#abstract = "abs".into();
        p.full_text = full_text.map(str::to_string);
        paper_repo.save(&p).expect("save paper");
        p.id
    }

    #[test]
    fn get_annotated_paper_roundtrip_includes_offsets_and_quote() {
        let db = fresh_db();
        let pid = save_paper(
            &db,
            "p-1",
            "Neutron stars",
            Some("Neutron stars are dense."),
        );
        let repo = SqliteAnnotationRepository::new(db.clone());
        let mut ann = Annotation::new_root(
            pid.clone(),
            "lars".into(),
            "key claim".into(),
            Anchor {
                char_range: Some((0, 13)),
                quote: Some("Neutron stars".into()),
                prefix: None,
                suffix: Some(" are dense.".into()),
                ..Anchor::default()
            },
        );
        ann.tags = vec!["physics".into()];
        repo.create(&ann).expect("create ann");

        let json = build_annotated_paper(&db, "p-1").expect("build");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(v["paper"]["id"], "p-1");
        assert_eq!(v["paper"]["full_text"], "Neutron stars are dense.");
        let arr = v["annotations"].as_array().expect("annotations array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["anchor"]["quote"], "Neutron stars");
        assert_eq!(arr[0]["anchor"]["char_range"][0], 0);
        assert_eq!(arr[0]["anchor"]["char_range"][1], 13);
        assert_eq!(arr[0]["root_id"], arr[0]["id"]);
        assert!(arr[0]["parent_id"].is_null());
    }

    #[test]
    fn get_annotated_paper_excludes_soft_deleted() {
        let db = fresh_db();
        let pid = save_paper(&db, "p-2", "T", None);
        let repo = SqliteAnnotationRepository::new(db.clone());
        let live = Annotation::new_root(
            pid.clone(),
            "lars".into(),
            "live".into(),
            Anchor {
                quote: Some("q1".into()),
                ..Anchor::default()
            },
        );
        let doomed = Annotation::new_root(
            pid,
            "lars".into(),
            "gone".into(),
            Anchor {
                quote: Some("q2".into()),
                ..Anchor::default()
            },
        );
        repo.create(&live).unwrap();
        repo.create(&doomed).unwrap();
        repo.soft_delete(doomed.id.as_str()).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&build_annotated_paper(&db, "p-2").unwrap()).unwrap();
        let arr = v["annotations"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["note"], "live");
    }

    #[test]
    fn get_annotated_paper_zero_annotations_returns_empty_array() {
        let db = fresh_db();
        save_paper(&db, "p-3", "Empty", None);
        let v: serde_json::Value =
            serde_json::from_str(&build_annotated_paper(&db, "p-3").unwrap()).unwrap();
        assert_eq!(v["annotations"].as_array().unwrap().len(), 0);
        assert!(v["source_version"].is_null());
    }

    #[test]
    fn get_annotated_paper_replies_carry_root_id() {
        let db = fresh_db();
        let pid = save_paper(&db, "p-4", "T", None);
        let repo = SqliteAnnotationRepository::new(db.clone());
        let root = Annotation::new_root(
            pid,
            "lars".into(),
            "root note".into(),
            Anchor {
                quote: Some("q".into()),
                ..Anchor::default()
            },
        );
        let reply = Annotation::new_reply(&root, "claude".into(), "agreed".into());
        repo.create(&root).unwrap();
        repo.create(&reply).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&build_annotated_paper(&db, "p-4").unwrap()).unwrap();
        let arr = v["annotations"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let reply_entry = arr
            .iter()
            .find(|a| a["parent_id"].is_string())
            .expect("has reply");
        assert_eq!(reply_entry["root_id"], root.id.as_str());
    }

    #[test]
    fn get_annotated_paper_unknown_paper_errors() {
        let db = fresh_db();
        let err = build_annotated_paper(&db, "nope").unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn truncate_abstract_shorter_than_limit_untouched() {
        let (out, truncated) = truncate_abstract("short text", 100);
        assert_eq!(out, "short text");
        assert!(!truncated);
    }

    #[test]
    fn truncate_abstract_over_limit_appends_ellipsis() {
        let input = "a".repeat(120);
        let (out, truncated) = truncate_abstract(&input, 100);
        assert!(truncated);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), 103);
    }

    #[test]
    fn truncate_abstract_respects_multibyte_boundary() {
        // U+2019 is 3 bytes; the truncation must slice on char boundaries.
        let input = "D\u{2019}Ippolito ".repeat(60);
        let (out, _) = truncate_abstract(&input, 50);
        assert_eq!(out.chars().count(), 53);
    }

    #[test]
    fn strips_tags_and_script() {
        let html = "<html><body><p>Hello <b>world</b>.</p><script>var x=1;</script></body></html>";
        let out = html_to_text(html);
        assert!(out.contains("Hello"));
        assert!(out.contains("world"));
        assert!(!out.contains("var x"));
    }

    #[test]
    fn source_registry_covers_every_adapter_name_used_by_build_adapters_full() {
        // These are the names accepted by scitadel_adapters::build_adapters_full;
        // the registry must stay in lockstep so the MCP tool stays honest.
        let expected = [
            "pubmed",
            "arxiv",
            "openalex",
            "inspire",
            "patentsview",
            "lens",
            "epo",
        ];
        for name in expected {
            assert!(
                SOURCE_REGISTRY.iter().any(|s| s.name == name),
                "missing registry entry for adapter {name}"
            );
        }
    }

    #[test]
    fn configured_flips_on_credential_presence() {
        let mut config = Config::default();
        config.openalex.api_key.clear();
        let oa = SOURCE_REGISTRY
            .iter()
            .find(|s| s.name == "openalex")
            .unwrap();
        assert!(!is_source_configured(oa, &config));
        config.openalex.api_key = "lars@example.org".into();
        assert!(is_source_configured(oa, &config));
    }

    #[test]
    fn sources_without_credentials_are_always_configured() {
        let config = Config::default();
        for name in ["arxiv", "inspire"] {
            let src = SOURCE_REGISTRY.iter().find(|s| s.name == name).unwrap();
            assert!(
                is_source_configured(src, &config),
                "{name} should always be configured"
            );
        }
    }
}
