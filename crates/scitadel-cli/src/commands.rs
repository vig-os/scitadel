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
    let server = scitadel_mcp::server::ScitadelServer::new();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}

pub fn tui(theme_override: Option<&str>) -> Result<()> {
    let config = load_config();
    let email = config.openalex.api_key.clone();
    let papers_dir = config.papers_dir();
    let reader = std::env::var("USER").unwrap_or_else(|_| "unknown".into());
    // Resolve theme before any rendering so the very first frame uses
    // the right palette. Order: --theme > SCITADEL_THEME > ui.theme >
    // auto-detect (#137).
    let resolved = scitadel_tui::theme::resolve(theme_override, &config.ui.theme);
    scitadel_tui::theme::init(resolved);
    scitadel_tui::run(
        &config.db_path,
        email,
        papers_dir,
        config.ui.show_institutional_hint,
        reader,
    )?;
    Ok(())
}

/// Options for the `init` wizard — forwarded from the CLI.
#[derive(Debug, Default)]
pub struct InitOptions {
    pub db_path: Option<PathBuf>,
    pub email: Option<String>,
    pub sources: Option<Vec<String>>,
    /// Non-interactive: never prompt. Missing values keep their existing
    /// or default config value silently.
    pub yes: bool,
}

pub fn init(opts: InitOptions) -> Result<()> {
    let mut config = load_config();

    // Resolve db path early so we can report it at the end.
    if let Some(p) = opts.db_path.clone() {
        config.db_path = p;
    }

    // Collect values to write: CLI flags first, then interactive prompts,
    // then fall back to whatever load_config() resolved.
    let interactive = !opts.yes && std::io::IsTerminal::is_terminal(&std::io::stdin());

    let email = opts
        .email
        .or_else(|| {
            if interactive && config.openalex.api_key.is_empty() {
                prompt_line(
                    "OpenAlex / Unpaywall email (used for OA PDF lookups, recommended)",
                    "",
                )
            } else {
                None
            }
        })
        .unwrap_or_else(|| config.openalex.api_key.clone());

    let sources = opts
        .sources
        .or_else(|| {
            if interactive {
                let joined = config.default_sources.join(",");
                prompt_line("Enabled sources (comma-separated)", &joined).map(parse_sources_csv)
            } else {
                None
            }
        })
        .unwrap_or_else(|| config.default_sources.clone());

    config.openalex.api_key.clone_from(&email);
    config.default_sources.clone_from(&sources);

    let config_path = config_path_for_db(&config.db_path);
    write_config_toml(&config_path, &config)
        .with_context(|| format!("failed to write config to {}", config_path.display()))?;

    // Always init the DB last so the config points at something real.
    let db = Database::open(&config.db_path).context("failed to open database")?;
    db.migrate().context("migration failed")?;

    println!();
    println!("  Config written: {}", config_path.display());
    println!("  Database:       {}", config.db_path.display());
    if !email.is_empty() {
        println!("  OA email:       {email}");
    }
    println!("  Sources:        {}", sources.join(", "));

    let keyed_sources_needed: Vec<&str> = sources
        .iter()
        .filter_map(|s| match s.as_str() {
            "patentsview" | "lens" | "epo" => Some(s.as_str()),
            _ => None,
        })
        .collect();
    if !keyed_sources_needed.is_empty() {
        println!();
        println!("  Credentials still needed for:");
        for s in &keyed_sources_needed {
            println!("    - {s} (run: scitadel auth login {s})");
        }
    }

    println!();
    println!(
        "  Try it: scitadel search \"machine learning\" --sources {}",
        sources.join(",")
    );
    Ok(())
}

/// Read one line from stdin, showing `prompt [default]:`. Returns `None` if
/// the user hits enter without input and `default` is empty; otherwise the
/// trimmed input or the default.
fn prompt_line(prompt: &str, default: &str) -> Option<String> {
    use std::io::{BufRead, Write};
    let mut out = std::io::stdout();
    if default.is_empty() {
        let _ = write!(out, "  {prompt}: ");
    } else {
        let _ = write!(out, "  {prompt} [{default}]: ");
    }
    let _ = out.flush();

    let stdin = std::io::stdin();
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_err() {
        return None;
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        if default.is_empty() {
            None
        } else {
            Some(default.to_string())
        }
    } else {
        Some(trimmed.to_string())
    }
}

/// Parse a comma-separated source list, trimming whitespace and dropping blanks.
fn parse_sources_csv(s: String) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Config file lives next to the DB under a `.scitadel/` directory.
fn config_path_for_db(db_path: &std::path::Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("config.toml")
}

/// Write a minimal, human-editable config.toml. We only write the fields
/// the user explicitly set so defaults can continue to evolve in-code.
fn write_config_toml(path: &std::path::Path, config: &scitadel_core::config::Config) -> Result<()> {
    use std::fmt::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut out = String::new();
    out.push_str("# scitadel config — generated by `scitadel init`. Edit freely.\n\n");
    let sources = config
        .default_sources
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "default_sources = [{sources}]").unwrap();
    if !config.openalex.api_key.is_empty() {
        out.push_str("\n[openalex]\n");
        writeln!(out, "api_key = \"{}\"", config.openalex.api_key).unwrap();
    }
    std::fs::write(path, out)?;
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

    let result = downloader.download(doi, &out_dir).await;

    // Persist outcome on the matching paper row (#112) before reporting
    // so the Papers table reflects the attempt regardless of outcome.
    persist_cli_download_outcome(&config, doi, result.as_ref().ok());

    let result = result.context("download failed")?;

    println!("  Format: {}", result.format);
    println!("  Source: {}", result.source);
    println!("  Access: {}", result.access);
    println!("  Size:   {} bytes", result.bytes);
    println!("  Saved:  {}", result.path.display());

    Ok(())
}

fn persist_cli_download_outcome(
    config: &scitadel_core::config::Config,
    doi: &str,
    success: Option<&scitadel_adapters::download::DownloadResult>,
) {
    use scitadel_adapters::download::AccessStatus;
    use scitadel_core::models::DownloadStatus;
    use scitadel_core::ports::PaperRepository as _;

    let db = match scitadel_db::sqlite::Database::open(&config.db_path) {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(error = %e, "could not open DB to persist download outcome");
            return;
        }
    };
    if let Err(e) = db.migrate() {
        tracing::warn!(error = %e, "DB migration failed while persisting download outcome");
        return;
    }
    let (paper_repo, _, _, _, _) = db.repositories();
    let paper = match paper_repo.find_by_doi(doi) {
        Ok(Some(p)) => p,
        Ok(None) => {
            tracing::debug!(doi, "no paper row for DOI; skipping download-state write");
            return;
        }
        Err(e) => {
            tracing::warn!(doi, error = %e, "DOI lookup failed");
            return;
        }
    };

    let (path, status) = match success {
        Some(r) => {
            let ds = match r.access {
                AccessStatus::FullText => DownloadStatus::Downloaded,
                AccessStatus::Abstract | AccessStatus::Paywall | AccessStatus::Unknown => {
                    DownloadStatus::Paywall
                }
            };
            (Some(r.path.to_string_lossy().into_owned()), ds)
        }
        None => (None, DownloadStatus::Failed),
    };
    if let Err(e) = paper_repo.update_download_state(paper.id.as_str(), path.as_deref(), status) {
        tracing::warn!(error = %e, "failed to persist download outcome");
    }
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

/// `scitadel bib import` — parse + match + persist a `.bib` file.
/// Surfaces a per-paper summary line and a final tally; under
/// `--verbose`, also prints dropped `keywords=` and `file=` fields.
pub fn bib_import(
    path: &std::path::Path,
    strategy: &str,
    reader: Option<String>,
    verbose: bool,
) -> Result<()> {
    use std::fmt::Write as _;

    use scitadel_db::sqlite::{
        SqliteAnnotationRepository, SqlitePaperAliasRepository, SqlitePaperRepository,
    };
    use scitadel_export::import::{MergeAction, MergeStrategy};
    use scitadel_mcp::bib_import::{ImportOptions, import_bibtex_file};

    let strategy = MergeStrategy::parse(strategy).ok_or_else(|| {
        anyhow::anyhow!("unknown --strategy: {strategy}; valid: reject, db-wins, bib-wins, merge")
    })?;
    let reader =
        reader.unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "import".into()));

    let db = open_db()?;
    let papers = SqlitePaperRepository::new(db.clone());
    let aliases = SqlitePaperAliasRepository::new(db.clone());
    let annotations = SqliteAnnotationRepository::new(db);

    let options = ImportOptions {
        strategy,
        reader,
        lenient: true,
    };
    let report = import_bibtex_file(path, &options, &papers, &aliases, &annotations)
        .with_context(|| format!("import {}", path.display()))?;

    for row in &report.rows {
        let id_short = row
            .paper_id
            .as_deref()
            .map_or_else(|| "—".into(), |s| s.chars().take(8).collect::<String>());
        let action = match row.action {
            MergeAction::Created => "created",
            MergeAction::Updated => "updated",
            MergeAction::Unchanged => "unchanged",
            MergeAction::Rejected => "rejected",
        };
        let mut line = format!("  {action:<9} {id_short}  {}", row.citekey);
        if !row.from_bib.is_empty() {
            let _ = write!(line, " — bib:[{}]", row.from_bib.join(","));
        }
        if !row.kept_from_db.is_empty() {
            let _ = write!(line, " — kept_db:[{}]", row.kept_from_db.join(","));
        }
        if row.annotation_created {
            line.push_str(" + annotation");
        }
        println!("{line}");
        if verbose {
            if !row.dropped_keywords.is_empty() {
                println!("    dropped keywords: {}", row.dropped_keywords.join(", "));
            }
            if let Some(f) = &row.dropped_file {
                println!("    dropped file: {f}");
            }
        }
    }
    println!(
        "\nimported {} entries: {} created, {} updated, {} unchanged, {} rejected, {} failed",
        report.rows.len(),
        report.count(MergeAction::Created),
        report.count(MergeAction::Updated),
        report.count(MergeAction::Unchanged),
        report.count(MergeAction::Rejected),
        report.failed.len(),
    );
    Ok(())
}

/// `scitadel bib rekey` — reassign a paper's citation key.
/// Prints the `old → new` mapping so users can `sed` their
/// manuscripts. Fails loudly on collision; logs the op for audit.
pub fn bib_rekey(paper_id: &str, key: Option<&str>, reader: Option<String>) -> Result<()> {
    use scitadel_db::sqlite::{SqlitePaperAliasRepository, SqlitePaperRepository};
    use scitadel_mcp::bib_rekey::{RekeyError, rekey_paper};

    let reader = reader.unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "rekey".into()));
    let db = open_db()?;
    let papers = SqlitePaperRepository::new(db.clone());
    let aliases = SqlitePaperAliasRepository::new(db);

    // Allow id-prefix resolution like other CLI commands.
    let resolved_id = {
        let all = papers.list_all(10_000, 0)?;
        let matches: Vec<&scitadel_core::models::Paper> = all
            .iter()
            .filter(|p| p.id.as_str().starts_with(paper_id))
            .collect();
        match matches.len() {
            0 => bail!("no paper matches id prefix '{paper_id}'"),
            1 => matches[0].id.as_str().to_string(),
            n => bail!("ambiguous paper id prefix '{paper_id}' — matches {n} records"),
        }
    };

    match rekey_paper(&papers, &aliases, &resolved_id, key, &reader) {
        Ok(out) => {
            if out.changed {
                println!(
                    "rekeyed {}: {} → {}",
                    &out.paper_id[..out.paper_id.len().min(8)],
                    out.old_key.as_deref().unwrap_or("<none>"),
                    out.new_key,
                );
                if let Some(old) = out.old_key {
                    println!(
                        "  old key preserved as alias; existing citations \\cite{{{old}}} still resolve"
                    );
                }
            } else {
                println!(
                    "rekey was a no-op — paper {} already has key '{}'",
                    &out.paper_id[..out.paper_id.len().min(8)],
                    out.new_key,
                );
            }
            Ok(())
        }
        Err(RekeyError::PaperNotFound(id)) => bail!("paper '{id}' not found"),
        Err(RekeyError::KeyCollision { key, owner }) => bail!(
            "citation key '{key}' is already used by paper {} — pick a different key or rekey that paper first",
            &owner[..owner.len().min(8)],
        ),
        Err(RekeyError::InvalidKey(k)) => bail!(
            "invalid citation key '{k}': must start with a letter and contain only letters, digits, '-', '_', or ':'"
        ),
        Err(RekeyError::Core(e)) => Err(e.into()),
    }
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
