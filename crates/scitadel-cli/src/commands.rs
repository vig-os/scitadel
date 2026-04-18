use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use scitadel_core::config::load_config;
use scitadel_core::credentials;
use scitadel_core::models::{ResearchQuestion, SearchTerm};
use scitadel_core::ports::{
    AssessmentRepository, PaperRepository, QuestionRepository, SearchRepository,
};
use scitadel_db::sqlite::Database;

fn open_db() -> Result<Database> {
    let config = load_config();
    let db = Database::open(&config.db_path).context("failed to open database")?;
    db.migrate().context("migration failed")?;
    Ok(db)
}

/// Resolve an ID by prefix match against a list.
fn resolve_prefix<'a, T, F>(items: &'a [T], prefix: &str, get_id: F) -> Result<&'a T>
where
    F: Fn(&T) -> &str,
{
    let matches: Vec<&T> = items
        .iter()
        .filter(|item| get_id(item).starts_with(prefix))
        .collect();
    match matches.len() {
        0 => bail!("no match for prefix '{prefix}'"),
        1 => Ok(matches[0]),
        n => bail!("ambiguous prefix '{prefix}' — matches {n} records"),
    }
}

pub async fn mcp() -> Result<()> {
    use rmcp::ServiceExt;
    let transport = rmcp::transport::io::stdio();
    let server = scitadel_mcp::server::ScitadelServer;
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}

pub fn tui() -> Result<()> {
    let config = load_config();
    let email = config.openalex.api_key.clone();
    let papers_dir = config.papers_dir();
    scitadel_tui::run(
        &config.db_path,
        email,
        papers_dir,
        config.ui.show_institutional_hint,
    )?;
    Ok(())
}

pub fn init(db_path: Option<PathBuf>) -> Result<()> {
    let config = load_config();
    let path = db_path.unwrap_or(config.db_path);
    let db = Database::open(&path).context("failed to open database")?;
    db.migrate().context("migration failed")?;
    println!("Database initialized at: {}", path.display());
    Ok(())
}

pub async fn search(
    query: Option<String>,
    sources: String,
    max_results: usize,
    question_id: Option<String>,
) -> Result<()> {
    let config = load_config();
    let db = Database::open(&config.db_path).context("failed to open database")?;
    db.migrate().context("migration failed")?;
    let (paper_repo, search_repo, q_repo, _, _) = db.repositories();

    let mut parameters = serde_json::Map::new();
    let mut query = query;

    // Resolve question-driven query
    if let Some(ref qid) = question_id {
        let question = if let Some(q) = q_repo.get_question(qid)? {
            q
        } else {
            let questions = q_repo.list_questions()?;
            resolve_prefix(&questions, qid, |q| q.id.as_str())?.clone()
        };

        parameters.insert(
            "question_id".into(),
            serde_json::Value::String(question.id.as_str().to_string()),
        );

        if query.is_none() {
            let terms = q_repo.get_terms(question.id.as_str())?;
            if terms.is_empty() {
                bail!(
                    "No search terms linked to question '{}'. Add terms first.",
                    question.id.short()
                );
            }
            let q: String = terms
                .iter()
                .filter(|t| !t.query_string.is_empty())
                .map(|t| t.query_string.as_str())
                .collect::<Vec<_>>()
                .join(" OR ");
            if q.is_empty() {
                bail!("Linked search terms have no query strings.");
            }
            println!("  Auto-built query from {} term group(s)", terms.len());
            query = Some(q);
        }
    }

    let query = query.context("Provide a QUERY argument or use --question")?;
    let source_list: Vec<String> = sources.split(',').map(|s| s.trim().to_string()).collect();

    let adapters = scitadel_adapters::build_adapters_full(
        &source_list,
        &config.pubmed.api_key,
        &config.openalex.api_key,
        &config.patentsview.api_key,
        &config.lens.api_key,
        &config.epo.consumer_key,
        &config.epo.consumer_secret,
    )
    .context("failed to build adapters")?;

    println!("Searching {} for: {query}", source_list.join(", "));

    let (mut search_record, candidates) =
        scitadel_core::services::orchestrator::run_search(&query, &adapters, max_results, 3).await;

    search_record.parameters = serde_json::Value::Object({
        let mut p: serde_json::Map<String, serde_json::Value> =
            if let serde_json::Value::Object(m) = search_record.parameters {
                m
            } else {
                serde_json::Map::new()
            };
        p.extend(parameters);
        p
    });

    println!("  Sources queried: {}", search_record.source_outcomes.len());
    for outcome in &search_record.source_outcomes {
        let icon = if outcome.status == scitadel_core::models::SourceStatus::Success {
            "+"
        } else {
            "!"
        };
        print!(
            "  [{icon}] {}: {} results ({:.0}ms)",
            outcome.source, outcome.result_count, outcome.latency_ms
        );
        if let Some(ref err) = outcome.error {
            print!(" - {err}");
        }
        println!();
    }
    println!("  Total candidates: {}", search_record.total_candidates);

    let (papers, mut search_results) =
        scitadel_core::services::dedup::deduplicate(&candidates, 0.85);
    search_record.total_papers = papers.len() as i32;
    println!("  Unique papers after dedup: {}", papers.len());

    // Resolve against existing DB records by DOI
    let mut id_map = std::collections::HashMap::new();
    for paper in &papers {
        if let Some(ref doi) = paper.doi
            && let Ok(Some(existing)) = paper_repo.find_by_doi(doi)
            && existing.id != paper.id
        {
            id_map.insert(
                paper.id.as_str().to_string(),
                existing.id.as_str().to_string(),
            );
        }
    }

    paper_repo.save_many(&papers)?;
    search_repo.save(&search_record)?;

    for sr in &mut search_results {
        sr.search_id = search_record.id.clone();
        if let Some(new_id) = id_map.get(sr.paper_id.as_str()) {
            sr.paper_id = scitadel_core::models::PaperId::from(new_id.as_str());
        }
    }
    search_repo.save_results(&search_results)?;

    println!("\n  Search ID: {}", search_record.id);
    println!("  Results saved to: {}", config.db_path.display());

    Ok(())
}

pub fn history(limit: i64) -> Result<()> {
    let db = open_db()?;
    let (_, search_repo, _, _, _) = db.repositories();

    let searches = search_repo.list_searches(limit)?;
    if searches.is_empty() {
        println!("No search history found.");
        return Ok(());
    }

    for s in &searches {
        let success_count = s
            .source_outcomes
            .iter()
            .filter(|o| o.status == scitadel_core::models::SourceStatus::Success)
            .count();
        println!(
            "  {}  {}  \"{}\"  {} papers  {}/{} sources ok",
            s.id.short(),
            s.created_at.format("%Y-%m-%d %H:%M"),
            s.query,
            s.total_papers,
            success_count,
            s.source_outcomes.len()
        );
    }

    Ok(())
}

pub fn show(id: &str) -> Result<()> {
    let db = open_db()?;
    let (paper_repo, _, _, _, _) = db.repositories();

    // Try as paper ID first
    if let Ok(Some(paper)) = paper_repo.get(id) {
        let json = serde_json::to_string_pretty(&paper)?;
        println!("{json}");
        return Ok(());
    }

    // Try prefix match
    let all = paper_repo.list_all(1000, 0)?;
    let paper = resolve_prefix(&all, id, |p| p.id.as_str())?;
    let json = serde_json::to_string_pretty(paper)?;
    println!("{json}");
    Ok(())
}

pub fn export(search_id: &str, format: &str, output: Option<PathBuf>) -> Result<()> {
    let db = open_db()?;
    let (paper_repo, search_repo, _, _, _) = db.repositories();

    let search = if let Some(s) = search_repo.get(search_id)? {
        s
    } else {
        let searches = search_repo.list_searches(100)?;
        resolve_prefix(&searches, search_id, |s| s.id.as_str())?.clone()
    };

    let results = search_repo.get_results(search.id.as_str())?;
    let paper_ids: std::collections::HashSet<&str> =
        results.iter().map(|r| r.paper_id.as_str()).collect();
    let papers: Vec<_> = paper_ids
        .iter()
        .filter_map(|id| paper_repo.get(id).ok().flatten())
        .collect();

    let content = match format {
        "json" => scitadel_export::export_json(&papers, 2),
        "csv" => scitadel_export::export_csv(&papers),
        "bibtex" => scitadel_export::export_bibtex(&papers),
        _ => bail!("unknown format: {format}"),
    };

    if let Some(path) = output {
        std::fs::write(&path, &content)?;
        println!("Exported {} papers to {}", papers.len(), path.display());
    } else {
        println!("{content}");
    }

    Ok(())
}

pub fn diff(search_a: &str, search_b: &str) -> Result<()> {
    let db = open_db()?;
    let (_, search_repo, _, _, _) = db.repositories();

    let (added, removed) = search_repo.diff_searches(search_a, search_b)?;
    println!("Added: {} papers", added.len());
    for id in &added {
        println!("  + {}", &id[..id.len().min(8)]);
    }
    println!("Removed: {} papers", removed.len());
    for id in &removed {
        println!("  - {}", &id[..id.len().min(8)]);
    }

    Ok(())
}

pub fn question_create(text: &str, description: &str) -> Result<()> {
    let db = open_db()?;
    let (_, _, q_repo, _, _) = db.repositories();

    let mut q = ResearchQuestion::new(text);
    q.description = description.to_string();
    q_repo.save_question(&q)?;

    println!("  Question ID: {}", q.id);
    println!("  Text: {text}");
    Ok(())
}

pub fn question_list() -> Result<()> {
    let db = open_db()?;
    let (_, _, q_repo, _, _) = db.repositories();

    let questions = q_repo.list_questions()?;
    if questions.is_empty() {
        println!("No research questions found.");
        return Ok(());
    }

    for q in &questions {
        println!(
            "  {}  {}  \"{}\"",
            q.id.short(),
            q.created_at.format("%Y-%m-%d %H:%M"),
            q.text
        );
    }

    Ok(())
}

pub fn question_add_terms(
    question_id: &str,
    terms: &[String],
    query_string: Option<String>,
) -> Result<()> {
    let db = open_db()?;
    let (_, _, q_repo, _, _) = db.repositories();

    let question = if let Some(q) = q_repo.get_question(question_id)? {
        q
    } else {
        let questions = q_repo.list_questions()?;
        resolve_prefix(&questions, question_id, |q| q.id.as_str())?.clone()
    };

    let query_str = query_string.unwrap_or_else(|| terms.join(" "));

    let mut term = SearchTerm::new(question.id.clone());
    term.terms = terms.to_vec();
    term.query_string.clone_from(&query_str);
    q_repo.save_term(&term)?;

    println!(
        "  Terms added to question {}: {:?}",
        question.id.short(),
        terms
    );
    println!("  Query string: {query_str}");
    Ok(())
}

pub async fn assess(
    search_id: &str,
    question_id: &str,
    model: &str,
    temperature: f64,
    scorer_backend: &str,
) -> Result<()> {
    let db = open_db()?;
    let (paper_repo, search_repo, q_repo, a_repo, _) = db.repositories();

    let search = if let Some(s) = search_repo.get(search_id)? {
        s
    } else {
        let searches = search_repo.list_searches(100)?;
        resolve_prefix(&searches, search_id, |s| s.id.as_str())?.clone()
    };

    let question = if let Some(q) = q_repo.get_question(question_id)? {
        q
    } else {
        let questions = q_repo.list_questions()?;
        resolve_prefix(&questions, question_id, |q| q.id.as_str())?.clone()
    };

    let results = search_repo.get_results(search.id.as_str())?;
    let paper_ids: std::collections::HashSet<&str> =
        results.iter().map(|r| r.paper_id.as_str()).collect();
    let papers: Vec<_> = paper_ids
        .iter()
        .filter_map(|id| paper_repo.get(id).ok().flatten())
        .collect();

    println!(
        "Scoring {} papers against: \"{}\"",
        papers.len(),
        question.text
    );
    println!("  Model: {model}  Temperature: {temperature}  Backend: {scorer_backend}");

    let backend = match scorer_backend {
        "cli" => scitadel_scoring::ScorerBackend::Cli,
        "api" => scitadel_scoring::ScorerBackend::Api,
        _ => scitadel_scoring::ScorerBackend::Auto,
    };

    let options = scitadel_scoring::ScoringOptions {
        backend,
        model: model.to_string(),
        temperature,
    };

    let scorer = scitadel_scoring::create_scorer(options)
        .await
        .context("failed to create scorer")?;

    let mut assessments = Vec::new();
    let total = papers.len();

    for (i, paper) in papers.iter().enumerate() {
        match scorer.score_paper(paper, &question).await {
            Ok(assessment) => {
                println!(
                    "  [{}/{}] {:.2}  {}",
                    i + 1,
                    total,
                    assessment.score,
                    &paper.title[..paper.title.len().min(60)]
                );
                assessments.push(assessment);
            }
            Err(e) => {
                println!("  [{}/{}] FAIL  {}", i + 1, total, e);
                assessments.push(scitadel_core::models::Assessment {
                    id: scitadel_core::models::AssessmentId::new(),
                    paper_id: paper.id.clone(),
                    question_id: question.id.clone(),
                    score: 0.0,
                    reasoning: format!("Scoring failed: {e}"),
                    model: Some(model.to_string()),
                    prompt: None,
                    temperature: Some(temperature),
                    assessor: format!("{model}:error"),
                    created_at: chrono::Utc::now(),
                });
            }
        }
    }

    for a in &assessments {
        a_repo.save(a)?;
    }

    let all_scores: Vec<f64> = assessments.iter().map(|a| a.score).collect();
    let avg = if all_scores.is_empty() {
        0.0
    } else {
        all_scores.iter().sum::<f64>() / all_scores.len() as f64
    };
    let relevant = all_scores.iter().filter(|&&s| s >= 0.6).count();

    println!("\n  Scored: {} papers", assessments.len());
    println!("  Average relevance: {avg:.2}");
    println!("  Relevant (>=0.6): {relevant}/{}", assessments.len());

    Ok(())
}

pub async fn download(doi: &str, output_dir: Option<PathBuf>) -> Result<()> {
    let config = load_config();
    let out_dir = output_dir.unwrap_or_else(|| config.papers_dir());

    let downloader =
        scitadel_adapters::download::PaperDownloader::new(config.openalex.api_key.clone(), 60.0);

    println!("Downloading paper: {doi}");
    println!("  Output dir: {}", out_dir.display());

    let result = downloader
        .download(doi, &out_dir)
        .await
        .context("download failed")?;

    println!("  Format: {}", result.format);
    println!("  Source: {}", result.source);
    println!("  Access: {}", result.access);
    println!("  Size:   {} bytes", result.bytes);
    println!("  Saved:  {}", result.path.display());

    Ok(())
}

#[allow(clippy::unnecessary_wraps)]
pub fn snowball(
    _search_id: &str,
    _question_id: &str,
    _depth: i32,
    _threshold: f64,
    _direction: &str,
    _model: &str,
) -> Result<()> {
    // Snowball requires OpenAlex citation fetcher which needs the full openalex module
    // This is a stub that will be completed when the snowball service is ported
    println!("Snowball command is not yet implemented in the Rust version.");
    println!("Use the Python version for snowballing: python -m scitadel snowball ...");
    Ok(())
}

pub fn auth_login(source: &str) -> Result<()> {
    let creds = find_source_credentials(source)?;

    println!("Storing credentials for '{source}' in system keychain.");

    for key in creds.keys {
        let value = prompt_credential(key.label, key.secret)?;
        credentials::store(key.keychain_key, &value).map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("  Stored: {}", key.keychain_key);
    }

    println!("Done. Credentials saved to system keychain.");
    Ok(())
}

pub fn auth_logout(source: &str) -> Result<()> {
    let creds = find_source_credentials(source)?;

    for key in creds.keys {
        match credentials::delete(key.keychain_key) {
            Ok(()) => println!("  Removed: {}", key.keychain_key),
            Err(e) => println!("  Skip: {} ({e})", key.keychain_key),
        }
    }

    println!("Credentials for '{source}' removed.");
    Ok(())
}

pub fn auth_status() -> Result<()> {
    println!("Source credentials status:\n");

    for creds in credentials::ALL_SOURCES {
        let status = match credentials::check_source(creds) {
            Ok(()) => "configured",
            Err(_) => "not configured",
        };

        let icon = if status == "configured" { "+" } else { "-" };
        println!("  [{icon}] {:<14} {status}", creds.source);

        for key in creds.keys {
            let loc = if credentials::get_keychain(key.keychain_key).is_some() {
                "keychain"
            } else if std::env::var(key.env_var)
                .ok()
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            {
                "env"
            } else {
                "missing"
            };
            println!("      {}: {loc}", key.label);
        }
    }

    println!("\nSources without credentials (no auth needed):");
    println!("  [+] arxiv");
    Ok(())
}

fn find_source_credentials(source: &str) -> Result<&'static credentials::SourceCredentials> {
    credentials::ALL_SOURCES
        .iter()
        .find(|c| c.source == source)
        .copied()
        .ok_or_else(|| {
            let names: Vec<&str> = credentials::ALL_SOURCES.iter().map(|c| c.source).collect();
            anyhow::anyhow!("Unknown source '{source}'. Available: {}", names.join(", "))
        })
}

fn prompt_credential(label: &str, secret: bool) -> Result<String> {
    if secret {
        // Read without echo
        print!("  {label}: ");
        std::io::stdout().flush()?;
        let value = rpassword::read_password().context("failed to read password")?;
        Ok(value)
    } else {
        print!("  {label}: ");
        std::io::stdout().flush()?;
        let mut value = String::new();
        std::io::stdin().read_line(&mut value)?;
        Ok(value.trim().to_string())
    }
}
