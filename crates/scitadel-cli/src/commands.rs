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

/// Print every theme advertised by `scitadel-tui::theme::Theme::registry`
/// in `name — description` form. Pure stdout + exit-0 — meant as a
/// discovery aid for `--theme` (#137).
pub fn list_themes() -> Result<()> {
    let entries = scitadel_tui::theme::Theme::registry();
    let width = entries.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    println!("Available themes (use with --theme or set ui.theme in config.toml):");
    for (name, desc) in entries {
        println!("  {name:<width$}  {desc}", width = width);
    }
    println!();
    println!("Resolution order: --theme flag > SCITADEL_THEME env > [ui] theme in config > auto.");
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
    let (resolved, label) =
        scitadel_tui::theme::resolve_with_label(theme_override, &config.ui.theme);
    scitadel_tui::theme::init(resolved);
    let toast = format!("theme: {label}");
    scitadel_tui::run(
        &config.db_path,
        email,
        papers_dir,
        config.ui.show_institutional_hint,
        reader,
        Some(toast),
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

    // Theme prompt (#137). Default `auto` keeps the existing dark
    // behaviour when COLORFGBG is unset; users on light terminals get
    // an automatically-readable palette without per-machine config.
    let theme = if interactive {
        let current = config.ui.theme.clone();
        prompt_line(
            "TUI theme (auto|light|dark|dalton-dark|dalton-light)",
            &current,
        )
        .map(|s| sanitize_theme_input(&s))
        .unwrap_or(current)
    } else {
        config.ui.theme.clone()
    };

    config.openalex.api_key.clone_from(&email);
    config.default_sources.clone_from(&sources);
    config.ui.theme.clone_from(&theme);

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
    println!("  Theme:          {theme}");

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

/// Normalise free-form theme input from the init wizard to a value the
/// resolver understands. Unknown strings fall back to `auto` rather
/// than being written verbatim — `[ui] theme = "dalton-pink"` would
/// silently fold to Auto at TUI launch anyway, and a typo in the
/// config file is harder to debug than one caught at write time.
fn sanitize_theme_input(s: &str) -> String {
    let trimmed = s.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "auto" | "dark" | "light" | "dalton-dark" | "dalton-bright" | "dalton-light" => trimmed,
        _ => "auto".into(),
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
    // Only persist `ui.theme` when it diverges from the default so
    // users who never picked a non-default value keep an empty `[ui]`
    // section out of their config (#137).
    if config.ui.theme != "auto" {
        out.push_str("\n[ui]\n");
        writeln!(out, "theme = \"{}\"", config.ui.theme).unwrap();
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
    use scitadel_db::sqlite::SqlitePaperTagRepository;

    let db = open_db()?;
    let (paper_repo, search_repo, _, _, _) = db.repositories();
    let tag_repo = SqlitePaperTagRepository::new(db);

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
        "bibtex" => scitadel_export::export_bibtex_with_tags(&papers, |id| {
            tag_repo.tags_for(id).unwrap_or_default()
        }),
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
        SqlitePaperTagRepository,
    };
    use scitadel_export::import::{MergeAction, MergeStrategy};
    use scitadel_mcp::bib_import::{ImportOptions, import_bibtex_file};

    let strategy = MergeStrategy::parse(strategy).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown --strategy: {strategy}; valid: reject, db-wins, bib-wins, merge, interactive"
        )
    })?;
    let reader =
        reader.unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "import".into()));

    let db = open_db()?;
    let papers = SqlitePaperRepository::new(db.clone());
    let aliases = SqlitePaperAliasRepository::new(db.clone());
    let annotations = SqliteAnnotationRepository::new(db.clone());
    let tags = SqlitePaperTagRepository::new(db);

    let options = ImportOptions {
        strategy,
        reader,
        lenient: true,
        // CLI does not yet wire stdin prompts; #161's TUI/CLI
        // prompt-surface is out of scope. Without a resolver,
        // `--strategy interactive` degrades to the same per-row
        // failure path as `--strategy merge` for ambiguous-alias
        // rows. The trait is now in place for follow-ups.
        prompt_resolver: None,
    };
    let report = import_bibtex_file(path, &options, &papers, &aliases, &annotations, &tags)
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
        if !row.paper_tags_written.is_empty() {
            let _ = write!(line, " + {} tag(s)", row.paper_tags_written.len());
        }
        println!("{line}");
        if verbose {
            if !row.paper_tags_written.is_empty() {
                println!("    paper tags: {}", row.paper_tags_written.join(", "));
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

/// `scitadel bib watch <question_id>` — long-running snapshot.
/// Polls SQLite, debounces bursts, hash-and-skips no-op writes,
/// flushes pending change on SIGINT/SIGTERM.
pub async fn bib_watch(
    question_id: &str,
    output: &std::path::Path,
    reader: Option<String>,
    min_score: Option<f64>,
    debounce_ms: u64,
    poll_ms: u64,
) -> Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use scitadel_core::ports::QuestionRepository;
    use scitadel_db::sqlite::{
        SqliteAssessmentRepository, SqlitePaperRepository, SqliteQuestionRepository,
        SqliteShortlistRepository,
    };
    use scitadel_mcp::bib_watch::{WatchOptions, run_watch_loop};

    let reader = reader.unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "watch".into()));
    let db = open_db()?;
    let papers = SqlitePaperRepository::new(db.clone());
    let assessments = SqliteAssessmentRepository::new(db.clone());
    let shortlist = SqliteShortlistRepository::new(db.clone());
    let questions = SqliteQuestionRepository::new(db);

    // Validate the question id (prefix-resolve like other CLI ops) so
    // a typo doesn't silently watch nothing.
    let resolved_question_id = {
        let all = questions.list_questions()?;
        let matches: Vec<&scitadel_core::models::ResearchQuestion> = all
            .iter()
            .filter(|q| q.id.as_str().starts_with(question_id))
            .collect();
        match matches.len() {
            0 => bail!("no question matches id prefix '{question_id}'"),
            1 => matches[0].id.as_str().to_string(),
            n => bail!("ambiguous question id prefix '{question_id}' — matches {n} records"),
        }
    };

    let opts = WatchOptions {
        question_id: resolved_question_id.clone(),
        reader,
        output: output.to_path_buf(),
        debounce: Duration::from_millis(debounce_ms),
        poll_interval: Duration::from_millis(poll_ms),
        min_score,
    };

    println!(
        "watching question {} → {} (debounce={}ms, poll={}ms{}); press Ctrl-C to stop",
        &resolved_question_id[..resolved_question_id.len().min(8)],
        output.display(),
        debounce_ms,
        poll_ms,
        match min_score {
            Some(s) => format!(", min_score={s}"),
            None => String::new(),
        },
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    let signal_flag = Arc::clone(&shutdown);
    tokio::spawn(async move {
        // Listen for both Ctrl-C and SIGTERM (e.g. systemd stop).
        let mut sigterm =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to install SIGTERM handler; Ctrl-C only");
                    let _ = tokio::signal::ctrl_c().await;
                    signal_flag.store(true, Ordering::SeqCst);
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
        signal_flag.store(true, Ordering::SeqCst);
    });

    run_watch_loop(opts, papers, assessments, shortlist, shutdown).await?;
    println!("watch stopped — final snapshot flushed if pending");
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

// ---------- bib snapshot / verify (#178) ----------

/// Sidecar suffix appended to the output `.bib` path. One sidecar per
/// `.bib` keeps scope to a single (question, output_file) — see #178
/// "Sidecar location collision" pitfall.
pub const SIDECAR_SUFFIX: &str = ".scitadel-bib.lock";

fn sidecar_path_for(bib: &std::path::Path) -> PathBuf {
    let mut s = bib.as_os_str().to_owned();
    s.push(SIDECAR_SUFFIX);
    PathBuf::from(s)
}

fn current_reader() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".into())
}

/// Load the question's shortlist once: returns the resolved question,
/// the shortlist's paper IDs (in shortlist insertion order), the
/// `Paper` records, and a paper_id → tags map. Snapshot and verify
/// share this code path so they can never disagree about what the
/// shortlist *is* at this moment in time.
fn load_shortlist(
    question_prefix: &str,
    reader: &str,
) -> Result<(
    scitadel_core::models::ResearchQuestion,
    Vec<String>,
    Vec<scitadel_core::models::Paper>,
    std::collections::HashMap<String, Vec<String>>,
)> {
    use scitadel_db::sqlite::{SqlitePaperTagRepository, SqliteShortlistRepository};

    let db = open_db()?;
    let (paper_repo, _, q_repo, _, _) = db.repositories();
    let shortlist_repo = SqliteShortlistRepository::new(db.clone());
    let tag_repo = SqlitePaperTagRepository::new(db);

    let question = if let Some(q) = q_repo.get_question(question_prefix)? {
        q
    } else {
        let questions = q_repo.list_questions()?;
        resolve_prefix(&questions, question_prefix, |q| q.id.as_str())?.clone()
    };

    let paper_ids = shortlist_repo
        .list(question.id.as_str(), reader)
        .context("failed to read shortlist")?;
    let papers: Vec<_> = paper_ids
        .iter()
        .filter_map(|id| paper_repo.get(id).ok().flatten())
        .collect();

    // Pre-load tags so the export closure isn't doing per-call I/O.
    let mut tags = std::collections::HashMap::new();
    for id in &paper_ids {
        let t = tag_repo.tags_for(id).unwrap_or_default();
        tags.insert(id.clone(), t);
    }

    Ok((question, paper_ids, papers, tags))
}

/// Render the shortlist's `.bib`. Centralized so snapshot and verify
/// produce byte-identical output for the same DB state — that's the
/// whole determinism story.
fn render_bibtex(
    papers: &[scitadel_core::models::Paper],
    tags: &std::collections::HashMap<String, Vec<String>>,
) -> String {
    scitadel_export::export_bibtex_with_tags(papers, |id| tags.get(id).cloned().unwrap_or_default())
}

/// Render the shortlist as canonical CSL-JSON 1.0.2 (#135 sub-feature
/// A). Same determinism contract as [`render_bibtex`] — same DB state ⇒
/// byte-identical output.
fn render_csl_json(
    papers: &[scitadel_core::models::Paper],
    tags: &std::collections::HashMap<String, Vec<String>>,
) -> String {
    scitadel_export::export_csl_json_with_tags(papers, |id| {
        tags.get(id).cloned().unwrap_or_default()
    })
}

/// Default output filename for a given snapshot format. Lets users skip
/// `--output` entirely for the common case while keeping the extension
/// honest (`.bib` for BibTeX, `.json` for CSL-JSON).
fn default_output_for(format: &str) -> &'static std::path::Path {
    match format {
        "csl-json" => std::path::Path::new("paper.json"),
        _ => std::path::Path::new("paper.bib"),
    }
}

pub fn bib_snapshot(
    question_prefix: &str,
    output: Option<&std::path::Path>,
    reader_arg: Option<&str>,
    no_lock: bool,
    format: &str,
) -> Result<()> {
    let reader = reader_arg.map_or_else(current_reader, str::to_string);
    let (question, paper_ids, papers, tags) = load_shortlist(question_prefix, &reader)?;

    let output_path = output.unwrap_or_else(|| default_output_for(format));

    let (content, lock) = match format {
        "csl-json" => {
            let c = render_csl_json(&papers, &tags);
            let l = scitadel_export::BibLockfile::new_csl_json(
                question.id.as_str(),
                &reader,
                &paper_ids,
                &c,
            );
            (c, l)
        }
        "bibtex" | "" => {
            let c = render_bibtex(&papers, &tags);
            let l = scitadel_export::BibLockfile::new_bibtex(
                question.id.as_str(),
                &reader,
                &paper_ids,
                &c,
            );
            (c, l)
        }
        other => bail!("unknown --format: {other}; valid: bibtex, csl-json"),
    };

    std::fs::write(output_path, &content)
        .with_context(|| format!("failed to write {}", output_path.display()))?;

    if no_lock {
        println!(
            "Wrote {} ({} papers) — sidecar skipped (--no-lock)",
            output_path.display(),
            papers.len()
        );
        return Ok(());
    }

    let sidecar = sidecar_path_for(output_path);
    std::fs::write(&sidecar, lock.to_json()?)
        .with_context(|| format!("failed to write {}", sidecar.display()))?;

    println!(
        "Wrote {} ({} papers) + {}",
        output_path.display(),
        papers.len(),
        sidecar.display()
    );
    Ok(())
}

/// Outcome of `bib verify`. Exit code follows `0/1/2` (ok/drift/stale).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// `.bib` and sidecar both match a fresh re-snapshot.
    Ok,
    /// shortlist or content changed since the lockfile.
    Drift {
        shortlist_changed: bool,
        content_changed: bool,
        diff: String,
    },
    /// Lockfile fields don't match the current binary, OR sidecar absent.
    Stale { reason: String },
}

impl VerifyOutcome {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::Drift { .. } => 1,
            Self::Stale { .. } => 2,
        }
    }
}

/// Cap a unified-style diff to `max_lines` so verify output stays
/// scannable. Anything longer is truncated with a sentinel line.
fn cap_diff(s: &str, max_lines: usize) -> String {
    use std::fmt::Write as _;
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max_lines {
        return s.to_string();
    }
    let mut out = lines[..max_lines].join("\n");
    let _ = write!(
        out,
        "\n... ({} more lines truncated)",
        lines.len() - max_lines
    );
    out
}

/// Tiny line-level diff. Real `diff -u` is overkill here — verify only
/// needs to *show* the user what moved, not produce a patch.
fn line_diff(old: &str, new: &str) -> String {
    use std::fmt::Write as _;
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut out = String::new();
    out.push_str("--- committed\n+++ regenerated\n");
    let max = old_lines.len().max(new_lines.len());
    for i in 0..max {
        match (old_lines.get(i), new_lines.get(i)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                let _ = writeln!(out, "-{a}");
                let _ = writeln!(out, "+{b}");
            }
            (Some(a), None) => {
                let _ = writeln!(out, "-{a}");
            }
            (None, Some(b)) => {
                let _ = writeln!(out, "+{b}");
            }
            (None, None) => break,
        }
    }
    out
}

/// Pure verify primitive — no I/O on shortlist, no DB. The CLI command
/// pulls the shortlist + sidecar + bib bytes and hands them off here so
/// tests can drive every exit-code branch from in-memory inputs.
pub fn verify_against_lockfile(
    bib_committed: &str,
    bib_regenerated: &str,
    paper_ids: &[String],
    lock: &scitadel_export::BibLockfile,
) -> VerifyOutcome {
    // Stale (algorithm or binary moved) is the most fundamental
    // failure: fail fast, the drift comparison would be nonsensical.
    let current_algo = scitadel_export::sidecar::ALGO_HASH;
    let current_version = env!("CARGO_PKG_VERSION");
    if lock.algo_hash != current_algo {
        return VerifyOutcome::Stale {
            reason: format!(
                "algo_hash mismatch: sidecar has {}, current binary has {} — \
                 the citation-key algorithm has moved (ADR-006). Regenerate.",
                short_hash(&lock.algo_hash),
                short_hash(current_algo),
            ),
        };
    }
    if lock.scitadel_version != current_version {
        return VerifyOutcome::Stale {
            reason: format!(
                "scitadel_version mismatch: sidecar has {}, current binary has {}. \
                 Regenerate to refresh.",
                lock.scitadel_version, current_version
            ),
        };
    }

    let shortlist_changed = scitadel_export::shortlist_hash(paper_ids) != lock.shortlist_hash;
    let content_changed = scitadel_export::content_hash(bib_committed) != lock.content_hash
        || bib_committed != bib_regenerated;
    if shortlist_changed || content_changed {
        let diff = cap_diff(&line_diff(bib_committed, bib_regenerated), 40);
        return VerifyOutcome::Drift {
            shortlist_changed,
            content_changed,
            diff,
        };
    }
    VerifyOutcome::Ok
}

fn short_hash(h: &str) -> String {
    // Strip optional `sha256:` prefix and keep first 12 chars for human
    // legibility — full hashes are noise in error messages.
    let bare = h.strip_prefix("sha256:").unwrap_or(h);
    if bare.len() <= 12 {
        bare.to_string()
    } else {
        format!("{}…", &bare[..12])
    }
}

/// Returns the exit code (0/1/2). main.rs forwards via `process::exit`.
pub fn bib_verify(
    file: &std::path::Path,
    question_override: Option<&str>,
    format: &str,
) -> Result<i32> {
    let bib_bytes =
        std::fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;

    let sidecar = sidecar_path_for(file);
    if !sidecar.exists() {
        let msg = format!(
            "no lockfile at {} — run `scitadel bib snapshot <question_id> --output {}` first",
            sidecar.display(),
            file.display()
        );
        if format == "json" {
            print_verify_json("stale", &msg, None);
        } else {
            eprintln!("STALE: {msg}");
        }
        return Ok(2);
    }
    let lock_bytes =
        std::fs::read_to_string(&sidecar).with_context(|| format!("read {}", sidecar.display()))?;
    let lock = scitadel_export::BibLockfile::from_json(&lock_bytes)
        .with_context(|| format!("parse {}", sidecar.display()))?;

    let question_id = question_override.unwrap_or(&lock.question_id);
    let (_q, paper_ids, papers, tags) = load_shortlist(question_id, &lock.reader)?;
    // Route to the matching emitter based on the sidecar's `format`
    // discriminant — that's the whole point of the field. Default to
    // BibTeX for backwards compat with sidecars written before #135.
    let regenerated = match lock.format.as_str() {
        scitadel_export::sidecar::FORMAT_CSL_JSON => render_csl_json(&papers, &tags),
        _ => render_bibtex(&papers, &tags),
    };

    let outcome = verify_against_lockfile(&bib_bytes, &regenerated, &paper_ids, &lock);
    let fix_line = if lock.format == scitadel_export::sidecar::FORMAT_CSL_JSON {
        format!(
            "scitadel bib snapshot {} --output {} --format csl-json",
            lock.question_id,
            file.display()
        )
    } else {
        format!(
            "scitadel bib snapshot {} --output {}",
            lock.question_id,
            file.display()
        )
    };
    match &outcome {
        VerifyOutcome::Ok => {
            if format == "json" {
                print_verify_json("ok", "matches lockfile", None);
            } else {
                println!("OK: {} matches lockfile", file.display());
            }
        }
        VerifyOutcome::Drift {
            shortlist_changed,
            content_changed,
            diff,
        } => {
            let label = match (shortlist_changed, content_changed) {
                (true, true) => "shortlist + content",
                (true, false) => "shortlist",
                (false, true) => "content",
                (false, false) => "lockfile",
            };
            if format == "json" {
                print_verify_json(
                    "drift",
                    &format!("{label} changed since lockfile"),
                    Some(diff),
                );
            } else {
                eprintln!("DRIFT: {label} changed since lockfile");
                eprintln!("{diff}");
                eprintln!("\nFix: {fix_line}");
            }
        }
        VerifyOutcome::Stale { reason } => {
            if format == "json" {
                print_verify_json("stale", reason, None);
            } else {
                eprintln!("STALE: {reason}");
                eprintln!("\nFix: {fix_line}");
            }
        }
    }
    Ok(outcome.exit_code())
}

fn print_verify_json(status: &str, message: &str, diff: Option<&str>) {
    let mut obj = serde_json::Map::new();
    obj.insert("status".into(), serde_json::Value::String(status.into()));
    obj.insert("message".into(), serde_json::Value::String(message.into()));
    if let Some(d) = diff {
        obj.insert("diff".into(), serde_json::Value::String(d.into()));
    }
    println!("{}", serde_json::Value::Object(obj));
}

// ---------- bib diff (#135 sub-feature C) ----------

/// Detect whether stdout is a TTY so we know whether ANSI color codes
/// will display correctly. Routed through `IsTerminal` from `std::io`
/// so we don't pull in a `colored`/`atty` crate just to ask the OS one
/// question. `--no-color` overrides this to `false` regardless.
fn stdout_is_tty() -> bool {
    use std::io::IsTerminal as _;
    std::io::stdout().is_terminal()
}

/// Run `scitadel bib diff` against two file paths OR a file vs. a
/// fresh-from-DB snapshot of `--question-id`. Returns the exit code:
/// `0` if there's no structural diff, `1` if there is (mirrors
/// `git diff` semantics so CI scripts can `if cmd; then` them).
pub fn bib_diff(
    file_a: &std::path::Path,
    file_b: Option<&std::path::Path>,
    question_id: Option<&str>,
    format: &str,
    no_color: bool,
    reader_arg: Option<&str>,
) -> Result<i32> {
    // Argument validation up front: exactly one of (file_b, question_id).
    let (entries_a, fmt_a) =
        scitadel_export::load_entries_from_path(file_a).map_err(|e| anyhow::anyhow!(e))?;
    let (entries_b, label_b) = match (file_b, question_id) {
        (Some(_), Some(_)) => bail!("pass either <file_b> OR --question-id, not both"),
        (None, None) => bail!("missing second side: pass <file_b> or --question-id <id>"),
        (Some(b), None) => {
            let (e, _fmt) =
                scitadel_export::load_entries_from_path(b).map_err(|err| anyhow::anyhow!(err))?;
            (e, format!("{}", b.display()))
        }
        (None, Some(qid)) => {
            // Fresh snapshot from DB. Use the same reader resolution
            // as snapshot/verify so stars and shortlist scoping line up.
            let reader = reader_arg.map_or_else(current_reader, str::to_string);
            let (q, _ids, papers, tags) = load_shortlist(qid, &reader)?;
            // Snapshot in the same flavor as the file side so the
            // comparison is apples-to-apples (the diff is structural so
            // either format works, but staying consistent avoids any
            // round-trip lossiness in the format-neutral lift).
            let content = match fmt_a {
                scitadel_export::BibFormat::CslJson => render_csl_json(&papers, &tags),
                scitadel_export::BibFormat::Bibtex => render_bibtex(&papers, &tags),
            };
            let (e, _fmt) = scitadel_export::load_entries_from_str(&content)
                .map_err(|err| anyhow::anyhow!(err))?;
            (e, format!("question {}", q.id.as_str()))
        }
    };

    let diff = scitadel_export::diff_entries(&entries_a, &entries_b);
    let exit = i32::from(!diff.is_empty());

    match format {
        "json" => {
            print!(
                "{}",
                scitadel_export::render_diff_json(&diff)
                    .context("failed to serialize diff JSON")?
            );
        }
        "text" | "" => {
            let use_color = !no_color && stdout_is_tty();
            let header_a = file_a.display().to_string();
            let text = scitadel_export::render_diff_text(&diff, &header_a, &label_b, use_color);
            print!("{text}");
        }
        other => bail!("unknown --format: {other}; valid: text, json"),
    }
    Ok(exit)
}

#[cfg(test)]
mod bib_diff_tests {
    //! Unit tests for the CLI's `bib diff` plumbing. The pure-logic
    //! tests live in `scitadel-export::diff::tests`; here we just
    //! sanity-check the wiring (TTY toggle, exit-code derivation,
    //! format dispatch).
    use scitadel_export::{BibDiff, ChangedEntry, Entry, FieldChange};

    fn no_diff() -> BibDiff {
        BibDiff::default()
    }

    fn with_diff() -> BibDiff {
        BibDiff {
            added: vec![],
            removed: vec![],
            changed: vec![ChangedEntry {
                citekey: "k".into(),
                before_citekey: None,
                field_changes: vec![FieldChange {
                    field: "title".into(),
                    before: Some("Old".into()),
                    after: Some("New".into()),
                }],
            }],
        }
    }

    #[test]
    fn empty_diff_renders_no_differences_marker() {
        let out = scitadel_export::render_diff_text(&no_diff(), "A", "B", false);
        assert!(out.contains("No differences"));
    }

    #[test]
    fn non_empty_diff_includes_changed_section() {
        let out = scitadel_export::render_diff_text(&with_diff(), "A", "B", false);
        assert!(out.contains("CHANGED (1):"));
        assert!(out.contains("Old → New"));
    }

    #[test]
    fn no_color_strips_ansi() {
        let out = scitadel_export::render_diff_text(&with_diff(), "A", "B", false);
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn color_path_emits_ansi() {
        let out = scitadel_export::render_diff_text(&with_diff(), "A", "B", true);
        assert!(out.contains('\x1b'));
    }

    #[test]
    fn json_format_round_trips_through_serde() {
        let d = with_diff();
        let json = scitadel_export::render_diff_json(&d).unwrap();
        let back: BibDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn diff_is_empty_helper_drives_exit_code() {
        // Simulate the `bib_diff` exit derivation: 0 iff is_empty, else 1.
        let _no_op = no_diff();
        assert_eq!(i32::from(!no_diff().is_empty()), 0);
        assert_eq!(i32::from(!with_diff().is_empty()), 1);
        // and the "summary one-liner" used in text rendering is robust
        // when authors empty.
        let bare = Entry {
            citekey: "x".into(),
            ..Default::default()
        };
        let mut d = BibDiff::default();
        d.added.push(bare);
        let out = scitadel_export::render_diff_text(&d, "A", "B", false);
        assert!(out.contains("ADDED (1):"));
    }
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

#[cfg(test)]
mod bib_verify_tests {
    use super::{VerifyOutcome, cap_diff, verify_against_lockfile};
    use scitadel_export::BibLockfile;
    use std::fmt::Write as _;

    fn fresh_lock(content: &str, paper_ids: &[String]) -> BibLockfile {
        BibLockfile::new_bibtex("q-1", "lars", paper_ids, content)
    }

    #[test]
    fn verify_returns_ok_when_committed_matches_lockfile() {
        let bib = "@article{a,\n  title = {A},\n}\n";
        let ids = vec!["p-1".to_string()];
        let lock = fresh_lock(bib, &ids);
        let outcome = verify_against_lockfile(bib, bib, &ids, &lock);
        assert_eq!(outcome, VerifyOutcome::Ok);
    }

    #[test]
    fn verify_returns_drift_when_content_differs() {
        let original = "@article{a,\n  title = {A},\n}\n";
        let modified = "@article{a,\n  title = {A — edited},\n}\n";
        let ids = vec!["p-1".to_string()];
        let lock = fresh_lock(original, &ids);
        let outcome = verify_against_lockfile(modified, original, &ids, &lock);
        match outcome {
            VerifyOutcome::Drift {
                content_changed,
                shortlist_changed,
                diff,
            } => {
                assert!(content_changed);
                assert!(!shortlist_changed);
                assert!(
                    diff.contains("--- committed") && diff.contains("+++ regenerated"),
                    "diff: {diff}"
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn verify_returns_drift_when_shortlist_differs() {
        let bib = "@article{a,\n  title = {A},\n}\n";
        let lock = fresh_lock(bib, &["p-1".into(), "p-2".into()]);
        let new_ids = vec!["p-1".to_string(), "p-2".into(), "p-3".into()];
        let outcome = verify_against_lockfile(bib, bib, &new_ids, &lock);
        match outcome {
            VerifyOutcome::Drift {
                shortlist_changed, ..
            } => {
                assert!(shortlist_changed);
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn verify_returns_stale_when_algo_hash_flips() {
        let bib = "@article{a,\n}\n";
        let ids = vec!["p-1".to_string()];
        let mut lock = fresh_lock(bib, &ids);
        lock.algo_hash = "deadbeef".into();
        let outcome = verify_against_lockfile(bib, bib, &ids, &lock);
        match &outcome {
            VerifyOutcome::Stale { reason } => {
                assert!(reason.contains("algo_hash"), "reason: {reason}");
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn verify_returns_stale_when_scitadel_version_flips() {
        let bib = "@article{a,\n}\n";
        let ids = vec!["p-1".to_string()];
        let mut lock = fresh_lock(bib, &ids);
        lock.scitadel_version = "0.0.0-time-traveler".into();
        let outcome = verify_against_lockfile(bib, bib, &ids, &lock);
        match outcome {
            VerifyOutcome::Stale { reason } => {
                assert!(reason.contains("scitadel_version"), "reason: {reason}");
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn verify_stale_takes_precedence_over_drift() {
        // If both algo_hash AND content disagree, we return stale —
        // drift comparisons are meaningless when the algorithm itself moved.
        let original = "@article{a,\n}\n";
        let modified = "@article{a, modified}\n";
        let ids = vec!["p-1".to_string()];
        let mut lock = fresh_lock(original, &ids);
        lock.algo_hash = "old-algo".into();
        let outcome = verify_against_lockfile(modified, original, &ids, &lock);
        assert!(matches!(outcome, VerifyOutcome::Stale { .. }));
    }

    #[test]
    fn diff_capping_preserves_head_and_marks_truncation() {
        let mut big = String::new();
        for i in 0..200 {
            let _ = writeln!(big, "line {i}");
        }
        let capped = cap_diff(&big, 40);
        assert!(capped.lines().count() <= 41);
        assert!(capped.contains("more lines truncated"));
    }
}
