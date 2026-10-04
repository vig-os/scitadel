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
    let openalex = config.openalex.auth();
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
        openalex,
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
            if interactive && config.openalex.email.is_empty() {
                prompt_line(
                    "OpenAlex / Unpaywall email (used for OA PDF lookups, recommended)",
                    "",
                )
            } else {
                None
            }
        })
        .unwrap_or_else(|| config.openalex.email.clone());

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

    config.openalex.email.clone_from(&email);
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

    // OpenAlex is in this list now: keyless requests are metered against
    // a shared per-IP daily budget and 429 once it's spent (#212).
    let keyed_sources_needed: Vec<&str> = sources
        .iter()
        .filter_map(|s| match s.as_str() {
            "patentsview" | "lens" | "epo" => Some(s.as_str()),
            "openalex" if config.openalex.api_key.is_empty() => Some(s.as_str()),
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
    // `email` is the polite-pool mailto; `api_key` is the real key. Never
    // write the key here — `scitadel auth login openalex` puts it in the
    // secret store, and config.toml is routinely committed (#212).
    if !config.openalex.email.is_empty() {
        out.push_str("\n[openalex]\n");
        writeln!(out, "email = \"{}\"", config.openalex.email).unwrap();
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
    field: &str,
) -> Result<()> {
    let config = load_config();
    let db = Database::open(&config.db_path).context("failed to open database")?;
    db.migrate().context("migration failed")?;
    let (paper_repo, search_repo, q_repo, _, _) = db.repositories();

    let openalex_field = scitadel_adapters::SearchField::parse(field)
        .map_err(|e| anyhow::anyhow!("--field: {e}"))?;

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

    let adapters = scitadel_adapters::build_adapters_with_field(
        &source_list,
        &config.pubmed.api_key,
        &config.openalex.auth(),
        &config.patentsview.api_key,
        &config.lens.api_key,
        &config.epo.consumer_key,
        &config.epo.consumer_secret,
        openalex_field,
    )
    .context("failed to build adapters")?;

    let field_note = match openalex_field {
        scitadel_adapters::SearchField::Any => String::new(),
        f => format!(" (openalex field={})", f.as_str()),
    };
    println!(
        "Searching {} for: {query}{field_note}",
        source_list.join(", ")
    );

    let (mut search_record, candidates) =
        scitadel_core::services::orchestrator::run_search(&query, &adapters, max_results, 3).await;

    // Record the OpenAlex field mode when the user picked a non-default —
    // makes `history` / re-runs reproducible without guessing what the
    // caller had set (#210).
    if openalex_field != scitadel_adapters::SearchField::default() {
        parameters.insert(
            "openalex_field".into(),
            serde_json::Value::String(openalex_field.as_str().to_string()),
        );
    }

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
        println!("{}", format_outcome_line(outcome));
    }
    let failed_sources = failed_source_names(&search_record);
    if !failed_sources.is_empty() {
        println!(
            "  [!] {} of {} source(s) failed ({}) — results below are incomplete.",
            failed_sources.len(),
            search_record.source_outcomes.len(),
            failed_sources.join(", ")
        );
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
        let failed = failed_source_names(s);
        let success_count = s.source_outcomes.len() - failed.len();
        // Name the sources that failed, so a run that silently lost a
        // whole source is visible in history rather than just short (#212).
        let failed_note = if failed.is_empty() {
            String::new()
        } else {
            format!("  [!] failed: {}", failed.join(", "))
        };
        println!(
            "  {}  {}  \"{}\"  {} papers  {}/{} sources ok{failed_note}",
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

/// One `Sources queried` line for a source outcome.
///
/// A failed source reports its error, never "0 results" — the two are not
/// the same thing, and collapsing them let a dead source pass for an empty
/// one in both human and agent workflows (#212).
fn format_outcome_line(outcome: &scitadel_core::models::SourceOutcome) -> String {
    if outcome.status == scitadel_core::models::SourceStatus::Success {
        format!(
            "  [+] {}: {} results ({:.0}ms)",
            outcome.source, outcome.result_count, outcome.latency_ms
        )
    } else {
        format!(
            "  [!] {}: {} ({:.0}ms)",
            outcome.source,
            outcome.error.as_deref().unwrap_or("unknown error"),
            outcome.latency_ms
        )
    }
}

/// Sources whose outcome in a search run was anything but success.
fn failed_source_names(search: &scitadel_core::models::Search) -> Vec<&str> {
    search
        .source_outcomes
        .iter()
        .filter(|o| o.status != scitadel_core::models::SourceStatus::Success)
        .map(|o| o.source.as_str())
        .collect()
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

    // An export built on a run where a source blew up is incomplete;
    // say so on stderr so it can't pass for a full sweep (#212).
    let failed = failed_source_names(&search);
    if !failed.is_empty() {
        eprintln!(
            "warning: search {} had {} failed source(s) ({}) — this export is incomplete.",
            search.id.short(),
            failed.len(),
            failed.join(", ")
        );
    }

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

/// `scitadel resolve-doi <doi>` — resolve a DOI to full metadata via
/// OpenAlex `/works/doi:<doi>` (#210).
///
/// Persists the resolved paper to the DB (skippable with `--no-save`)
/// so the follow-on flow (`show`, `download`, `assess`) can address it
/// by id. A malformed DOI is rejected without an HTTP round trip; a
/// well-formed DOI OpenAlex doesn't know about exits 1 with a clear
/// "not found" message rather than an empty JSON envelope.
pub async fn resolve_doi(doi: &str, json: bool, no_save: bool) -> Result<()> {
    use scitadel_core::ports::PaperRepository as _;

    let config = load_config();
    let adapter = scitadel_adapters::openalex::OpenAlexAdapter::new(config.openalex.auth(), 30.0);

    let paper = adapter
        .fetch_paper_by_doi(doi)
        .await
        .with_context(|| format!("OpenAlex lookup for DOI {doi} failed"))?
        .ok_or_else(|| anyhow::anyhow!("no OpenAlex record for DOI {doi}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&paper)?);
    } else {
        let authors = if paper.authors.is_empty() {
            "(none)".to_string()
        } else {
            paper.authors.join(", ")
        };
        println!("Resolved DOI: {}", paper.doi.as_deref().unwrap_or(doi));
        println!("  Title:    {}", paper.title);
        println!("  Authors:  {authors}");
        println!(
            "  Year:     {}",
            paper.year.map_or_else(|| "N/A".into(), |y| y.to_string())
        );
        println!("  Journal:  {}", paper.journal.as_deref().unwrap_or("N/A"));
        println!(
            "  OpenAlex: {}",
            paper.openalex_id.as_deref().unwrap_or("N/A")
        );
        println!("  Paper ID: {}", paper.id);
    }

    if !no_save {
        let db = open_db()?;
        let (paper_repo, _, _, _, _) = db.repositories();
        // If a row with this DOI already exists, keep its id so history /
        // annotations don't break; otherwise the resolved paper's own id
        // (the short OpenAlex id) becomes the canonical id.
        let existing = paper
            .doi
            .as_deref()
            .and_then(|d| paper_repo.find_by_doi(d).ok().flatten());
        if let Some(existing_id) = existing.as_ref().map(|p| p.id.clone()) {
            // Save under the existing id so downstream references (annotations,
            // search_results) keep resolving.
            let mut merged = paper.clone();
            merged.id = existing_id;
            paper_repo.save(&merged).context("save resolved paper")?;
            println!("  (updated existing DB row: {})", merged.id.short());
        } else {
            paper_repo.save(&paper).context("save resolved paper")?;
            println!("  (saved to DB as: {})", paper.id.short());
        }
    }

    Ok(())
}

pub async fn download(doi: &str, output_dir: Option<PathBuf>) -> Result<()> {
    let config = load_config();
    let out_dir = output_dir.unwrap_or_else(|| config.papers_dir());

    // Opened up front so the downloader paces against the same SQLite
    // ledger the rest of scitadel uses — two in-memory ledgers is the
    // twice-the-traffic failure ADR-007 §4 exists to prevent.
    let db = scitadel_db::sqlite::Database::open(&config.db_path)
        .context("open database for the pacing ledger")?;
    let downloader =
        scitadel_adapters::download::PaperDownloader::new(db, config.openalex.auth(), 60.0)
            .context("build the downloader")?;

    println!("Downloading paper: {doi}");
    println!("  Output dir: {}", out_dir.display());

    let result = downloader.download(doi, &out_dir).await;

    // Persist outcome on the matching paper row (#112) before reporting
    // so the Papers table reflects the attempt regardless of outcome.
    persist_cli_download_outcome(&config, doi, result.as_ref().ok());

    let result = result.context("download failed")?;

    println!("  Format: {}", result.format);
    println!("  Source: {}", result.source());
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
    let backend = credentials::backend();

    println!(
        "Storing credentials for '{source}' — backend: {}",
        backend.describe()
    );
    if backend == credentials::Backend::File {
        println!(
            "  Note: no OS secret service was found, so credentials go to a \
             plain-text file with mode 0600."
        );
    }

    for key in creds.keys {
        let label = if key.optional {
            format!("{} — press enter to skip", key.label)
        } else {
            key.label.to_string()
        };
        let value = prompt_credential(&label, key.secret)?;

        if value.is_empty() {
            if key.optional {
                println!("  Skipped: {}", key.store_key);
                continue;
            }
            bail!("{} is required for '{source}'", key.label);
        }

        credentials::store(key.store_key, &credentials::Secret::new(&value))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("  Stored: {}", key.store_key);
    }

    println!("Done. Credentials saved to the {backend} store.");
    Ok(())
}

pub fn auth_logout(source: &str) -> Result<()> {
    let creds = find_source_credentials(source)?;

    for key in creds.keys {
        match credentials::delete(key.store_key) {
            Ok(()) => println!("  Removed: {}", key.store_key),
            Err(e) => println!("  Skip: {} ({e})", key.store_key),
        }
    }

    println!("Credentials for '{source}' removed.");
    Ok(())
}

pub fn auth_status() -> Result<()> {
    println!("Credential store: {}\n", credentials::backend().describe());
    println!("Source credentials status:\n");

    for creds in credentials::ALL_SOURCES {
        let status = match credentials::check_source(creds) {
            Ok(()) => "configured",
            Err(_) => "not configured",
        };

        let icon = if status == "configured" { "+" } else { "-" };
        println!("  [{icon}] {:<14} {status}", creds.source);

        for key in creds.keys {
            // Location only — the value itself is never printed.
            println!("      {}: {}", key.label, credentials::key_location(key));
        }
    }

    println!("\nSources without credentials (no auth needed):");
    println!("  [+] arxiv");
    println!(
        "\nOverride the store with SCITADEL_CREDENTIAL_BACKEND=macos-keychain|secret-service|file."
    );
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

fn sidecar_path_for(bib: &std::path::Path) -> PathBuf {
    scitadel_export::sidecar_path_for(bib)
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

/// Render the shortlist's `.bib`. Kept as a thin wrapper around the
/// reusable rendering helper so verify (which compares against a
/// re-render) stays single-call-site.
fn render_bibtex(
    papers: &[scitadel_core::models::Paper],
    tags: &std::collections::HashMap<String, Vec<String>>,
) -> String {
    scitadel_export::export_bibtex_with_tags(papers, |id| tags.get(id).cloned().unwrap_or_default())
}

/// CSL-JSON sibling of [`render_bibtex`]. Used by `bib verify` to
/// re-render against the sidecar's recorded format.
fn render_csl_json(
    papers: &[scitadel_core::models::Paper],
    tags: &std::collections::HashMap<String, Vec<String>>,
) -> String {
    scitadel_export::export_csl_json_with_tags(papers, |id| {
        tags.get(id).cloned().unwrap_or_default()
    })
}

/// Parse the CLI's `--format` string into the typed enum the export
/// helper accepts. Centralized so the error message stays consistent
/// across snapshot + verify.
fn parse_format(format: &str) -> Result<scitadel_export::SnapshotFormat> {
    match format {
        "csl-json" => Ok(scitadel_export::SnapshotFormat::CslJson),
        "bibtex" | "" => Ok(scitadel_export::SnapshotFormat::BibTeX),
        other => bail!("unknown --format: {other}; valid: bibtex, csl-json"),
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
    let snapshot_format = parse_format(format)?;

    let output_path =
        output.unwrap_or_else(|| scitadel_export::default_filename_for_format(snapshot_format));

    let outcome = scitadel_export::write_snapshot(
        output_path,
        question.id.as_str(),
        &reader,
        &papers,
        &paper_ids,
        |id| tags.get(id).cloned().unwrap_or_default(),
        snapshot_format,
        !no_lock,
    )
    .with_context(|| format!("failed to write {}", output_path.display()))?;

    if let Some(sidecar) = outcome.sidecar_path {
        println!(
            "Wrote {} ({} papers) + {}",
            outcome.output_path.display(),
            outcome.entry_count,
            sidecar.display()
        );
    } else {
        println!(
            "Wrote {} ({} papers) — sidecar skipped (--no-lock)",
            outcome.output_path.display(),
            outcome.entry_count
        );
    }
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

/// `scitadel import-flat --paper <id> --root <dir>` — import a flat /
/// legacy directory tree of files as `artefacts` rows (ADR-007 §1
/// "Legacy data").
///
/// The paper is addressed by id prefix, like every other CLI command, so
/// the operator can paste the 8 characters `show` prints. Prints what was
/// recorded per kind, names anything it refused or could not place, and
/// exits non-zero when a path escaped the tree — a silent partial import
/// of a library is exactly the outcome worth making loud.
pub fn import_flat(paper: &str, root: &std::path::Path) -> Result<()> {
    use scitadel_adapters::import_flat::import_flat_tree;
    use scitadel_core::ports::PaperRepository as _;

    let db = open_db()?;
    let (paper_repo, _, _, _, _) = db.repositories();

    let resolved_id = {
        let all = paper_repo.list_all(10_000, 0)?;
        let matches: Vec<&scitadel_core::models::Paper> = all
            .iter()
            .filter(|p| p.id.as_str().starts_with(paper))
            .collect();
        match matches.len() {
            0 => bail!("no paper matches id prefix '{paper}'"),
            1 => matches[0].id.as_str().to_string(),
            n => bail!("ambiguous paper id prefix '{paper}' — matches {n} records"),
        }
    };
    let paper_row = paper_repo
        .get(&resolved_id)?
        .ok_or_else(|| anyhow::anyhow!("paper '{resolved_id}' not found"))?;

    let report = import_flat_tree(&db, &paper_row, root).map_err(|e| anyhow::anyhow!("{e}"))?;

    println!(
        "Imported {} artefact(s) for {} from {}",
        report.total(),
        paper_row.id.short(),
        report.paper_dir.display()
    );
    for (kind, n) in &report.counts {
        println!("  {kind:<16} {n}");
    }
    println!("  {:<16} {}", "blobs", report.blobs);
    if !report.gaps.is_empty() {
        println!("\n  Not present in this tree (recorded as wanted, not held):");
        for gap in &report.gaps {
            println!(
                "  + {} → drop the file at {}",
                gap.kind,
                gap.drop_path.display()
            );
        }
    }
    if !report.unrecognised.is_empty() {
        println!("\n  Left in place (no artefacts.kind fits):");
        for path in &report.unrecognised {
            println!("  - {}", path.display());
        }
    }
    if !report.refused.is_empty() {
        println!("\n  Refused (resolves outside the imported tree):");
        for path in &report.refused {
            println!("  - {}", path.display());
        }
        bail!(
            "{} path(s) escaped the imported tree and were not imported",
            report.refused.len()
        );
    }
    Ok(())
}

// ---------- coverage / action_list (ADR-007 §1 "Have", §2 "Status vocabulary") ----------

use scitadel_db::sqlite::{ALL_STATUSES, ALL_WANT_KINDS, ActionList, CoverageReport, PublisherKey};

/// The machine shape of `scitadel action_list --json`.
///
/// Carries the report the groups were partitioned *from* alongside the groups,
/// so a consumer can check the acceptance criterion itself:
/// `missing_total == report.missing_total == sum(group counts) + sum(deferred
/// counts)` without a second query and without trusting this file's arithmetic.
/// The machine shape of `scitadel coverage --json`.
///
/// The report verbatim, plus its three totals as fields rather than as something
/// a consumer has to re-derive — and the same shape `action_list --json`
/// answers with, so the acceptance criterion is checkable by comparing two JSON
/// documents and nothing else.
#[derive(serde::Serialize)]
struct CoverageJson<'a> {
    /// Recorded wants the ADR-007 §1 derivation says we do not hold.
    missing_total: usize,
    wanted_total: usize,
    held_total: usize,
    #[serde(flatten)]
    report: &'a CoverageReport,
}

#[derive(serde::Serialize)]
struct ActionListJson<'a> {
    /// [`CoverageReport::missing_total`] — the number `coverage` prints.
    missing_total: usize,
    /// Entries a human can act on: the sum of the group counts.
    human_total: usize,
    /// Entries left out of the human list, and why (see `deferred`).
    deferred_total: usize,
    /// `human_total + deferred_total`. Structurally equal to `missing_total`.
    accounted_total: usize,
    groups: &'a [scitadel_db::sqlite::ActionGroup],
    deferred: &'a [scitadel_db::sqlite::DeferredGroup],
    report: &'a CoverageReport,
}

/// `scitadel coverage [--kind <k>] [--json]` — what the library wants and does
/// not hold, per kind and per status (ADR-007 §1, §2).
///
/// "Have" is derived on read from `artefacts`; there is no stored have-status
/// and this command reads none. A work with no recorded want and no artefacts
/// is reported as untracked rather than counted as missing — no gap has been
/// recorded for it, and inventing one is the failure mode this report exists to
/// avoid (#260, #261, #275).
pub fn coverage(kind: Option<&str>, json: bool) -> Result<()> {
    // One read, one computation. `action_list` is the same value seen from the
    // other side; see [`action_list`].
    let list = read_action_list()?;
    // `coverage`'s projection of the same value `action_list` groups, so the
    // headline number it prints is the number the groups have to account for.
    let report = list.coverage(kind).map_err(|e| anyhow::anyhow!("{e}"))?;
    if json {
        let view = CoverageJson {
            missing_total: report.missing_total(),
            wanted_total: report.wanted_total(),
            held_total: report.held_total(),
            report: &report,
        };
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else {
        print!("{}", render_coverage(&report, list.accounted(), kind));
    }
    Ok(())
}

/// `scitadel action_list [--json]` — the missing entries grouped for a person,
/// by (action group, publisher) (ADR-007 §2).
///
/// Only what a human can act on is in the list. The statuses ADR-007 §2 marks
/// "—" — `pending`, `oa_fetchable`, `tdm_available`, `unavailable`,
/// `rate_limited`, `error` — are the `acquire` queue's, and every one of them
/// is listed below the groups with the ADR's own reason, because the counts
/// here sum to the `coverage` missing totals exactly and a reader has to be
/// able to see where the rest went.
pub fn action_list(json: bool) -> Result<()> {
    let list = read_action_list()?;
    if json {
        let view = ActionListJson {
            missing_total: list.report.missing_total(),
            human_total: list.human_total(),
            deferred_total: list.deferred_total(),
            accounted_total: list.accounted(),
            groups: &list.groups,
            deferred: &list.deferred,
            report: &list.report,
        };
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else {
        print!("{}", render_action_list(&list));
    }
    Ok(())
}

/// The one read both commands make.
fn read_action_list() -> Result<ActionList> {
    let db = open_db()?;
    db.action_list().map_err(|e| anyhow::anyhow!("{e}"))
}

/// `coverage`'s text projection.
///
/// `accounted` is [`ActionList::accounted`] rather than
/// [`CoverageReport::missing_total`] on purpose: the headline number a person
/// reads here is the one `action_list` has to match, so it is the number a
/// disagreement would show up in. `debug_assert_eq!` ties the two.
#[allow(clippy::too_many_lines)]
fn render_coverage(report: &CoverageReport, accounted: usize, kind: Option<&str>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(
        out,
        "Coverage — \"have\" is derived from `artefacts` (ADR-007 §1); nothing here is a stored status."
    );
    match kind {
        Some(kind) => {
            let _ = writeln!(
                out,
                "Restricted to --kind {kind}; the scope counters below stay library-wide."
            );
        }
        None => {
            let _ = writeln!(out);
        }
    }

    let _ = writeln!(out, "Scope");
    let _ = writeln!(
        out,
        "  {:<44} {:>6}   library-wide, not narrowed by --kind",
        "works in library", report.works_total
    );
    let _ = writeln!(
        out,
        "  {:<44} {:>6}   at least one recorded want",
        "works in acquisition scope", report.works_in_scope
    );
    let _ = writeln!(
        out,
        "  {:<44} {:>6}   no want, no artefacts: no gap is recorded, so not missing",
        "works untracked", report.untracked_works
    );
    let _ = writeln!(
        out,
        "  {:<44} {:>6}   files held, no want stated: neither covered nor missing",
        "works holding files nobody queued", report.held_untracked_works
    );

    let _ = writeln!(
        out,
        "\nBy kind — recorded wants, and how many the derivation closes"
    );
    let _ = writeln!(
        out,
        "  {:<18} {:>7} {:>8} {:>8}",
        "kind", "wanted", "missing", "held"
    );
    for kind in ALL_WANT_KINDS {
        let total = report.by_kind.get(kind).copied().unwrap_or_default();
        let _ = writeln!(
            out,
            "  {kind:<18} {:>7} {:>8} {:>8}",
            total.wanted, total.missing, total.held
        );
    }
    let _ = writeln!(
        out,
        "  {:<18} {:>7} {:>8} {:>8}",
        "total",
        report.wanted_total(),
        report.missing_total(),
        report.held_total()
    );

    let _ = writeln!(
        out,
        "\nBy status — missing entries only. A zero means no such papers, not \
         \"not tracked\": every status migration 013 allows appears."
    );
    for vocab in ALL_STATUSES {
        let count = report.by_status.get(vocab.status).copied().unwrap_or(0);
        let _ = writeln!(out, "  {:<20} {count:>5}   {}", vocab.status, vocab.meaning);
    }
    let _ = writeln!(out, "  {:<20} {:>5}", "sum", report.missing_total());

    if report.held_at_another_version > 0 {
        let _ = writeln!(
            out,
            "\n{} of the {} are works we DO hold a full text for, at a version the want does not \
             accept (ADR-007 §2 `wrong_version`).",
            report.held_at_another_version,
            report.missing_total()
        );
    }

    match kind {
        // Unfiltered, the two numbers are the same number — and that is
        // ADR-007 §2's acceptance criterion, so the assertion is here rather
        // than only in a test.
        None => {
            debug_assert_eq!(
                accounted,
                report.missing_total(),
                "action_list must account for every missing entry coverage reports"
            );
            let _ = writeln!(
                out,
                "\nMissing: {} of {} wanted ({} held). `scitadel action_list` accounts for the \
                 same {}.",
                report.missing_total(),
                report.wanted_total(),
                report.held_total(),
                report.missing_total()
            );
        }
        // Filtered, the two are different slices on purpose, so the line says
        // which is which rather than implying they should match.
        Some(_) => {
            let _ = writeln!(
                out,
                "\nMissing: {} of {} wanted in this slice ({} held). Unfiltered, \
                 `scitadel action_list` accounts for {accounted}.",
                report.missing_total(),
                report.wanted_total(),
                report.held_total()
            );
        }
    }

    if report.entries.is_empty() {
        let _ = writeln!(out, "\nNothing is missing from this slice of the library.");
        out.push_str(&render_identity_overrides(&report.identity_overrides));
        return out;
    }

    let _ = writeln!(out, "\nMissing entries ({})", report.entries.len());
    for entry in &report.entries {
        let _ = writeln!(
            out,
            "  {}  {}  {}",
            short_id(&entry.paper_id),
            entry.describe(),
            entry.status
        );
        if let Some(reason) = &entry.reason {
            let _ = writeln!(out, "      reason: {reason}");
        }
        // The publisher line comes from the DOI registry, never from a stored
        // name, so it cannot name a publisher that was never classified (#261).
        let key = PublisherKey::of(report.doi_of(&entry.paper_id));
        let _ = writeln!(out, "      publisher: {}", key.display());
        if let Some(note) = key.route_note(&entry.status, report.doi_of(&entry.paper_id)) {
            let _ = writeln!(out, "      route: {note}");
        }
        if let Some(hint) = &entry.hint_url {
            let _ = writeln!(out, "      hint: {hint}");
        }
        if let Some(drop) = &entry.drop_path {
            let _ = writeln!(out, "      drop: {drop}");
        }
    }
    out.push_str(&render_identity_overrides(&report.identity_overrides));
    out
}

/// The overrides a report carries, printed by **both** projections.
///
/// #253's escape hatch, and the reason it is printed rather than merely stored:
/// every other verdict in this codebase is chosen so a human can overturn it, and
/// an override nobody can see is indistinguishable from a bug. A work that
/// stopped blocking because a person checked the DOI by hand looks exactly like a
/// work that stopped blocking because the matcher changed — only the reason
/// distinguishes them, and the reason is the whole value of the flag.
///
/// Printed **after** the missing-entry tables and outside every total: an
/// override is neither a want nor an artefact, so counting it in either would
/// break the accounting ADR-007 §2 requires of `action_list`.
fn render_identity_overrides(overrides: &[scitadel_db::sqlite::IdentityOverride]) -> String {
    use std::fmt::Write as _;
    if overrides.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\nIdentity overridden by a person — {} work(s). The identity gate does not block \
         these, and no later run re-asks:",
        overrides.len()
    );
    for entry in overrides {
        let _ = writeln!(
            out,
            "  + {}  {}",
            short_id(&entry.paper_id),
            entry
                .phases
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        let _ = writeln!(out, "      reason: {}", entry.reason);
        let _ = writeln!(out, "      recorded: {}", entry.recorded_at);
    }
    out
}

/// `action_list`'s text projection.
fn render_action_list(list: &ActionList) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(
        out,
        "Action list — {} missing entries in {} group(s) a person can act on.",
        list.report.missing_total(),
        list.groups.len()
    );
    let _ = writeln!(
        out,
        "Same missing entries as `scitadel coverage`: {} accounted for, none dropped.",
        list.accounted()
    );
    debug_assert_eq!(list.accounted(), list.report.missing_total());

    if list.groups.is_empty() {
        let _ = writeln!(out, "\nNothing here needs a person.");
    }
    for group in &list.groups {
        let _ = writeln!(
            out,
            "\n{} — {} ({})",
            group.action,
            group.publisher.display(),
            group.count()
        );
        // The registry's own wording, so an unclassified publisher is never
        // reported as lacking a TDM route (#261).
        if let Some(note) = &group.route_note {
            let _ = writeln!(out, "  {note}");
        }
        for entry in &group.entries {
            let _ = writeln!(
                out,
                "  + {}  {}  [{}]",
                short_id(&entry.paper_id),
                entry.describe(),
                entry.status
            );
            if let Some(reason) = &entry.reason {
                let _ = writeln!(out, "      reason: {reason}");
            }
            if let Some(hint) = &entry.hint_url {
                let _ = writeln!(out, "      hint: {hint}");
            }
            if let Some(drop) = &entry.drop_path {
                let _ = writeln!(out, "      drop: {drop}");
            }
        }
    }

    let _ = writeln!(
        out,
        "\nNot a human action — {} entries, excluded from the groups above:",
        list.deferred_total()
    );
    let _ = writeln!(out, "  {:<20} {:>5}   goes to", "status", "count");
    for group in &list.deferred {
        let _ = writeln!(
            out,
            "  {:<20} {:>5}   {}",
            group.status, group.count, group.goes_to
        );
    }
    let _ = writeln!(out, "  {:<20} {:>5}", "total", list.deferred_total());

    out.push_str(&render_identity_overrides(&list.report.identity_overrides));

    let _ = writeln!(
        out,
        "\nAccounts for {} missing entries: {} human, {} deferred.",
        list.accounted(),
        list.human_total(),
        list.deferred_total()
    );
    out
}

/// The 8 characters every other scitadel surface prints for a work.
fn short_id(paper_id: &str) -> String {
    scitadel_core::models::PaperId::from(paper_id)
        .short()
        .to_string()
}

// ---------- override-identity (#253: a human overturns the identity gate) ----------

/// `scitadel override-identity <paper> --reason "<text>" [--json]`.
///
/// ADR-007 §3's "a mismatch blocks filing anything under that DOI" has one door
/// out of it, and this is it. Three things it deliberately does:
///
/// - **requires the reason.** `scitadel_db::sqlite::override_identity` refuses a
///   blank one, because `override_reason` is the only record that a person looked
///   at this work — an empty string would leave the row indistinguishable from a
///   bug.
/// - **covers both identity phases.** A person answering "is this the work I
///   asked for?" is answering once, not once per fetch step, and a flag that took
///   a phase argument would be a flag people forget.
/// - **returns a mismatched work to the `acquire` queue.** `identity_mismatch` is
///   ADR-007 §2's human-action status, so the work stays in `action_list` asking to
///   be checked until someone does. Leaving it there after they have would keep
///   asking forever.
///
/// Read-only apart from the override itself: no request, and nothing about the
/// want row is changed beyond the status transition the report names.
pub fn override_identity(paper: &str, reason: &str, json: bool) -> Result<()> {
    let config = load_config();
    let db = Database::open(&config.db_path).context("open database")?;
    db.migrate().context("migration failed")?;

    // Every other surface accepts an unambiguous prefix, so this one does too:
    // an id a tool printed must work on the way back in (#232).
    let paper_id = resolve_paper_id(&db, paper)?;

    let report = db
        .override_identity(&paper_id, reason)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_override_identity(&report));
    }
    Ok(())
}

/// The full id for an exact id or an unambiguous prefix.
fn resolve_paper_id(db: &Database, paper: &str) -> Result<String> {
    let (paper_repo, _, _, _, _) = db.repositories();
    if paper_repo.get(paper).is_ok() {
        return Ok(paper.to_string());
    }
    let matches: Vec<String> = paper_repo
        .list_all(10_000, 0)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_iter()
        .filter(|p| p.id.as_str().starts_with(paper))
        .map(|p| p.id.as_str().to_string())
        .collect();
    match matches.as_slice() {
        [] => Err(anyhow::anyhow!(
            "no work named {paper:?}; `scitadel show {paper}` lists what the library holds"
        )),
        // Naming one of two works would settle the wrong work's identity.
        [only] => Ok(only.clone()),
        many => Err(anyhow::anyhow!(
            "{paper:?} matches {} works ({}, …); use more of the id",
            many.len(),
            many[0]
        )),
    }
}

fn render_override_identity(report: &scitadel_db::sqlite::IdentityOverrideReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Identity settled for {} — the gate will not block it again.",
        report.paper_id
    );
    let _ = writeln!(
        out,
        "  phases:   {} (an override covers both; a later run does not re-ask)",
        report
            .phases
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(out, "  reason:   {}", report.reason);
    match (&report.gap_status_before, &report.gap_status_after) {
        (Some(before), Some(after)) => {
            let _ = writeln!(
                out,
                "  queue:    {before} -> {after} (back in the `acquire` queue; \
                 `scitadel acquire` will fetch it)"
            );
        }
        (Some(before), None) => {
            let _ = writeln!(out, "  queue:    {before}, unchanged");
        }
        _ => {
            let _ = writeln!(
                out,
                "  queue:    no want row recorded, so there was nothing to unblock"
            );
        }
    }
    let _ = writeln!(
        out,
        "\nShown by `scitadel coverage` and `scitadel action_list` under \
         \"Identity overridden by a person\"."
    );
    out
}

// ---------- acquire (ADR-007 §2 "Status vocabulary", §3 "Routes and the ladder")

/// What `acquire` was asked for. One struct so the flags cannot be re-ordered
/// into a different request by a call site.
#[derive(Debug, Default)]
pub struct AcquireOptions {
    /// `--from <queue.ndjson>`: import raid's acquisition queue first. Writes,
    /// so it is refused under `--dry-run`.
    pub from: Option<PathBuf>,
    /// `--dry-run`: plan only, no writes and no requests.
    pub dry_run: bool,
    /// `--resume`: only fetch what is due.
    pub resume: bool,
    pub kind: Option<String>,
    pub limit: Option<usize>,
}

/// `scitadel acquire [--dry-run] [--resume] [--kind] [--limit] [--json]`.
///
/// Drains the `acquire` queue — the `acquisition_state` rows ADR-007 §2 routes
/// to `acquire` — through the existing ladder. The decision of *which* works to
/// fetch is made from the derived "have" before anything is fetched, so a
/// second run over a library that already holds its full texts puts nothing on
/// the wire; see `scitadel_adapters::acquire`.
pub async fn acquire(opts: AcquireOptions, json: bool) -> Result<()> {
    let config = load_config();
    // Opened for the pacing ledger as well as the rows: the downloader spends
    // from the same SQLite ledger the rest of scitadel uses, and two in-memory
    // ledgers is the twice-the-traffic failure ADR-007 §4 exists to prevent.
    let db = Database::open(&config.db_path).context("open database for the pacing ledger")?;
    db.migrate().context("migration failed")?;

    let request = scitadel_adapters::acquire::AcquireRequest {
        paper_ids: Vec::new(),
        kind: opts.kind.clone(),
        limit: opts.limit,
        resume: opts.resume,
        dry_run: opts.dry_run,
    };

    // Built even for a dry run: it opens no socket and spends no permit, and
    // sharing one constructor with the real path is what keeps `--dry-run`
    // from being a second, subtly different code path.
    let downloader =
        scitadel_adapters::download::PaperDownloader::new(db.clone(), config.openalex.auth(), 60.0)
            .context("build the downloader")?;

    // `--from` imports, then drains: two reconciliations of two different things,
    // so they are two calls with one order. A queue import writes
    // `acquisition_state` rows, which is why `run_from_ndjson` refuses to do it
    // under `--dry-run` rather than quietly violating the flag.
    let report = match opts.from.as_deref() {
        Some(source) => {
            let (import, report) = scitadel_adapters::acquire::run_from_ndjson(
                &db,
                &downloader,
                &config.papers_dir(),
                &request,
                source,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "import": import,
                        "acquire": report,
                    }))?
                );
                return Ok(());
            }
            print!("{}", render_ndjson_import(&import));
            report
        }
        None => scitadel_adapters::acquire::run(&db, &downloader, &config.papers_dir(), &request)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_acquire(&report));
    }
    Ok(())
}

/// `acquire --from works.ndjson`'s import, as text.
///
/// The curation section is the one that must not be skimmed: it is the whole
/// point of the flag, and a reader who trusts that scitadel re-labelled raid's
/// curation verdicts would be wrong. So every preserved status is printed with
/// both strings — raid's own, and what scitadel stored — and the unknowns are
/// printed because a non-empty list means the queue's real shape differs from
/// what this build reads.
#[allow(clippy::too_many_lines)]
fn render_ndjson_import(report: &scitadel_adapters::acquire::NdjsonImport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Imported {} — raid's acquisition fields, curation statuses preserved verbatim.",
        report.source.display()
    );
    let _ = writeln!(
        out,
        "\n{:<10} {:>6} {:>6} {:>6} {:>6} {:>6}",
        "", "rows", "gaps", "kept", "kept?", "unknown"
    );
    let _ = writeln!(
        out,
        "{:<10} {:>6} {:>6} {:>6} {:>6} {:>6}",
        "read",
        report.rows_read,
        report.gaps_written,
        report.untouched_existing_gaps,
        report.preserved_statuses.len(),
        report.unknown.len()
    );

    if !report.preserved_statuses.is_empty() {
        let _ = writeln!(
            out,
            "\nCuration statuses — RAID's word, then scitadel's. Nothing was translated."
        );
        for status in &report.preserved_statuses {
            let stored = if status.in_reason {
                format!("{} (verbatim in reason)", status.stored_as)
            } else {
                format!("{} (raid's own ADR-007 §2 word)", status.stored_as)
            };
            let _ = writeln!(
                out,
                "  {}  {}/{}  \"{}\" → {}",
                short_id(&status.paper_id),
                status.kind,
                status.locator,
                status.raid_status,
                stored
            );
        }
        let _ = writeln!(
            out,
            "\n`pending` is a claim about SCITADEL — never tried by us — not a translation of \
             raid's verdict."
        );
    }
    if !report.unknown.is_empty() {
        let _ = writeln!(
            out,
            "\nNot in this library ({}): {}",
            report.unknown.len(),
            report.unknown.join(", ")
        );
        let _ = writeln!(
            out,
            "  No paper rows were created. `acquire_queue_add` treats an unknown id the same way."
        );
    }
    if !report.unsupported_kinds.is_empty() {
        let _ = writeln!(
            out,
            "\nArtefact kinds outside ADR-007 §2's vocabulary, not recorded:"
        );
        for (kind, n) in &report.unsupported_kinds {
            let _ = writeln!(out, "  {kind} × {n}");
        }
    }
    if !report.identity_disagreements.is_empty() {
        let _ = writeln!(
            out,
            "\nIdentity expectations that disagree with the stored work ({}). Not applied — \
             `scitadel override-identity` settles one by hand:",
            report.identity_disagreements.len()
        );
        for gap in &report.identity_disagreements {
            let _ = writeln!(
                out,
                "  {}  raid: \"{}\"  stored: \"{}\"",
                short_id(&gap.paper_id),
                gap.expected_title,
                gap.stored_title
            );
        }
    }
    if !report.unknown_fields.is_empty() {
        let _ = writeln!(
            out,
            "\nFields this build does not read ({}): {}",
            report.unknown_fields.len(),
            report
                .unknown_fields
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        let _ = writeln!(
            out,
            "  Reported, not dropped: #247's \"covers raid's queue files without loss\" is a \
             question about this list."
        );
    }
    out
}

// ---------- scan / attach / gc (ADR-007 §1 "Manual drop-ins", "Artefact rules")

/// The paper a `scan` / `attach` names, by id prefix like every other command.
///
/// Shared by both so the resolution rule — and its refusal on an ambiguous
/// prefix — is one implementation. Named by prefix because a person pastes the
/// eight characters `show` prints.
fn resolve_paper(
    paper: &str,
) -> Result<(scitadel_db::sqlite::Database, scitadel_core::models::Paper)> {
    use scitadel_core::ports::PaperRepository as _;

    let db = open_db()?;
    let (paper_repo, _, _, _, _) = db.repositories();
    let matches: Vec<scitadel_core::models::Paper> = paper_repo
        .list_all(10_000, 0)?
        .into_iter()
        .filter(|p| p.id.as_str().starts_with(paper))
        .collect();
    match matches.len() {
        0 => bail!("no paper matches id prefix '{paper}'"),
        // Naming one of two works would file the file under the wrong one, so
        // this is the caller's error to fix rather than a coin flip.
        1 => Ok((db, matches.into_iter().next().expect("one match"))),
        n => bail!("ambiguous paper id prefix '{paper}' — matches {n} records"),
    }
}

/// `scitadel scan --paper <id> [--root <dir>] [--dry-run] [--json]`.
pub fn scan(paper: &str, root: Option<&std::path::Path>, dry_run: bool, json: bool) -> Result<()> {
    let (db, paper_row) = resolve_paper(paper)?;
    if dry_run {
        let report = scitadel_adapters::scan::plan(&db, &paper_row, root)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", render_scan(&report, true));
        }
        return Ok(());
    }
    let report =
        scitadel_adapters::scan::scan(&db, &paper_row, root).map_err(|e| anyhow::anyhow!("{e}"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_scan(&report, false));
    }
    Ok(())
}

/// `scitadel attach <paper> <file> --kind <k> [--label] [--locator] [--json]`.
pub fn attach(
    paper: &str,
    file: &std::path::Path,
    kind: &str,
    label: Option<&str>,
    locator: Option<&str>,
    json: bool,
) -> Result<()> {
    let (db, paper_row) = resolve_paper(paper)?;
    let report = scitadel_adapters::scan::attach(&db, &paper_row, file, kind, label, locator)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    // A refusal is a non-zero exit **in both output modes**. Returning 0 with
    // `"refused_because": "bad_magic"` on stdout would put back exactly the silent
    // failure ADR-007 §1 measured: a landing page saved as an SI, in a script that
    // only checked the exit code.
    let refusal = report.file.refused_because.as_ref().map(|reason| {
        format!(
            "{} was not filed [{reason}]: {}",
            report.file.path.display(),
            report
                .file
                .detail
                .as_deref()
                .unwrap_or("(no reason recorded)")
        )
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_attach(&report));
    }
    if let Some(refusal) = refusal {
        bail!("{refusal}");
    }
    Ok(())
}

/// `scitadel gc [--dry-run] [--min-age-hours N] [--json]`.
pub fn gc(dry_run: bool, min_age_hours: i64, json: bool) -> Result<()> {
    let db = open_db()?;
    // `--dry-run` is a real flag rather than the default, and the report says
    // which of the two it is: "gc collected nothing" and "gc would collect
    // nothing" are different sentences.
    let report = db.collect_blobs(dry_run, min_age_hours)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_gc(&report));
    }
    // A refused exit code when there is nothing to do and no dry run is *not*
    // implied: `gc` succeeding means "the store is as it should be", whether
    // that is because it cleaned up or because there was nothing to clean.
    Ok(())
}

/// `scan`'s text projection.
///
/// The refusals are printed **before** the counts and are not summarised away: a
/// run that silently skipped seven landing pages looks exactly like a clean one,
/// and the 7-of-27 measurement in ADR-007 §1 is why that matters.
fn render_scan(report: &scitadel_adapters::scan::ScanReport, dry_run: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Scan{} — {} in {}",
        if dry_run { " (dry run)" } else { "" },
        report.paper_id_short,
        report.root.display()
    );
    if report.leased_elsewhere {
        let _ = writeln!(
            out,
            "\nAnother live process holds this work's lease, so nothing was read or written. \
             Run again once it finishes."
        );
        return out;
    }
    let refused: Vec<&scitadel_adapters::scan::FileOutcome> = report
        .files
        .iter()
        .filter(|f| f.refused_because.is_some())
        .collect();
    if !refused.is_empty() {
        let _ = writeln!(
            out,
            "\nNot filed ({}): each is recorded in `acquisition_attempts`, and no artefact row \
             was created.",
            refused.len()
        );
        for file in &refused {
            let _ = writeln!(
                out,
                "  [{}] {}  {}/{}",
                file.refused_because.as_deref().unwrap_or("?"),
                file.path.display(),
                file.kind,
                file.locator
            );
            if let Some(detail) = &file.detail {
                let _ = writeln!(out, "      {detail}");
            }
        }
    }
    if dry_run {
        let _ = writeln!(
            out,
            "\nWould file ({}), copy {} blob(s) into the store, and write no row.",
            report.files.len() - refused.len(),
            report.blobs
        );
        for file in report.files.iter().filter(|f| f.refused_because.is_none()) {
            let _ = writeln!(out, "  + {}  {}", file.kind, file.path.display());
        }
    } else {
        let _ = writeln!(out, "\nRecorded:");
        for (kind, n) in &report.counts {
            let _ = writeln!(out, "  {kind:<16} {n}");
        }
        let _ = writeln!(out, "  {:<16} {}", "blobs", report.blobs);
    }
    if !report.vanished.is_empty() {
        let _ = writeln!(
            out,
            "\nGone from disk ({}): flagged `missing_on_disk = 1`, the rows kept.",
            report.vanished.len()
        );
        for entry in &report.vanished {
            let _ = writeln!(out, "  - {entry}");
        }
    }
    if !report.identity.is_empty() {
        let _ = writeln!(out, "\nIdentity checks (ADR-007 §3, post-fetch):");
        for check in &report.identity {
            let _ = writeln!(
                out,
                "  {:<16} {}  {}",
                check.status,
                check.path.display(),
                check
                    .resolved_title
                    .as_deref()
                    .unwrap_or("(no title in the bytes)")
            );
        }
    }
    if !report.gaps.is_empty() {
        let _ = writeln!(out, "\nStill wanted (recorded, not held):");
        for gap in &report.gaps {
            let _ = writeln!(
                out,
                "  + {} → drop the file at {}",
                gap.kind,
                gap.drop_path.display()
            );
        }
    }
    match &report.manifest {
        Some(mirror) => {
            let _ = writeln!(out, "\nManifest: {}", mirror.path.display());
            let _ = writeln!(
                out,
                "  generated from the database after its commit, by the lease holder, through a \
                 rename. Never read back."
            );
        }
        None if dry_run => {
            let _ = writeln!(out, "\nManifest: not written (dry run).");
        }
        None => {
            let _ = writeln!(
                out,
                "\nManifest: not written. The database is authoritative; the next run \
                 regenerates it."
            );
        }
    }
    out
}

/// `attach`'s text projection.
///
/// A refusal is printed rather than thrown from here, so the report is complete
/// before the command exits non-zero — the caller owns the exit code, and it does
/// so in `--json` mode too. Returning 0 with `"refused_because": "bad_magic"` on
/// stdout would put back exactly the silent failure ADR-007 §1 measured: a landing
/// page saved as an SI, in a script that only checks the exit code.
fn render_attach(report: &scitadel_adapters::scan::AttachReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Attach — {}", report.paper_id_short);
    let _ = writeln!(
        out,
        "  {}  {}/{}",
        report.file.path.display(),
        report.file.kind,
        report.file.locator
    );
    if let Some(reason) = &report.file.refused_because {
        let _ = writeln!(
            out,
            "  NOT filed [{reason}] — recorded in `acquisition_attempts`."
        );
        if let Some(detail) = &report.file.detail {
            let _ = writeln!(out, "  {detail}");
        }
    }
    let _ = writeln!(out, "  Filed. route=manual, access_basis=manual.");
    if let Some(check) = &report.identity {
        let _ = writeln!(
            out,
            "  identity: {} — file says {:?}, work says {:?}",
            check.status,
            check.resolved_title.as_deref().unwrap_or("(none)"),
            check.expected_title.as_deref().unwrap_or("(none)")
        );
    }
    if report.retracted > 0 {
        let _ = writeln!(
            out,
            "  closed {} recorded gap(s) for this work.",
            report.retracted
        );
    }
    let _ = writeln!(
        out,
        "  provenance: {}",
        if report.inside_library {
            "inside this library, so the recorded path still resolves if the library moves"
        } else {
            "OUTSIDE this library — `imported_from` is an absolute path that will not move with              the library"
        }
    );
    if let Some(mirror) = &report.manifest {
        let _ = writeln!(out, "  manifest: {}", mirror.path.display());
    }
    out
}

/// `gc`'s text projection.
///
/// `bytes` shown for a dry run is what it *would* reclaim, and is labelled as
/// such: the two answers are the same number and mean different things.
fn render_gc(report: &scitadel_db::sqlite::GcReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "gc{} — unreferenced blobs (ADR-007 §1 \"Artefact rules\"). Min age: {}h.",
        if report.dry_run { " (dry run)" } else { "" },
        report.min_age_hours
    );
    let _ = writeln!(
        out,
        "\n{:<10} {:>8} {:>8} {:>8}",
        "blobs", "referenced", "unreferenced", "untracked files"
    );
    let _ = writeln!(
        out,
        "{:<10} {:>8} {:>8} {:>8}",
        "in store", report.blobs_total, report.referenced_total, report.untracked_files
    );

    if report.candidates.is_empty() {
        let _ = writeln!(
            out,
            "\nNothing {} collectable.",
            if report.dry_run { " would be" } else { " was" }
        );
    } else {
        let _ = writeln!(
            out,
            "\n{} ({}), {}:",
            if report.dry_run {
                "Would collect"
            } else {
                "Collected"
            },
            report.candidates.len(),
            human_bytes(report.reclaimable_bytes)
        );
        for candidate in &report.candidates {
            let _ = writeln!(
                out,
                "  {}  {:>10}  {}",
                &candidate.sha256[..candidate.sha256.len().min(12)],
                human_bytes(candidate.bytes),
                candidate.rel_path
            );
        }
    }
    if !report.skipped.is_empty() {
        let _ = writeln!(out, "\nKept ({}):", report.skipped.len());
        for skip in &report.skipped {
            let _ = writeln!(
                out,
                "  {}  {}",
                &skip.sha256[..12.min(skip.sha256.len())],
                skip.reason
            );
        }
    }
    if report.dry_run {
        let _ = writeln!(
            out,
            "\nNothing was deleted. Re-run without --dry-run to collect."
        );
    } else {
        let _ = writeln!(
            out,
            "\n{} row(s) deleted, {} file(s) removed, {} freed.",
            report.collected_rows,
            report.removed_files,
            human_bytes(report.freed_bytes())
        );
    }
    out
}

/// A byte count in the words a person reading a gc report needs.
fn human_bytes(bytes: i64) -> String {
    match bytes {
        b if b >= 1024 * 1024 * 1024 => format!("{:.1} GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{:.1} kB", b as f64 / 1024.0),
        b => format!("{b} bytes"),
    }
}

/// `acquire`'s text projection.
///
/// Two things here are not decoration. The **held** count is printed next to
/// the acquired count, because "acquired 0" reads like a failure until you can
/// see that the work was already done. And every publisher line is either a
/// label the DOI registry produced or the registry's own wording for why it
/// produced none — never a TDM verdict for a publisher nobody classified
/// (#261).
#[allow(clippy::too_many_lines)]
fn render_acquire(report: &scitadel_adapters::acquire::AcquireReport) -> String {
    use std::fmt::Write as _;
    let plan = &report.plan;
    let mut out = String::new();

    let _ = writeln!(
        out,
        "Acquire — \"have\" is derived from `artefacts` (ADR-007 §1), so nothing already held \
         is fetched."
    );
    if plan.dry_run {
        let _ = writeln!(
            out,
            "Dry run: no writes, no requests. Everything below is what would happen."
        );
    }
    if plan.resume {
        let _ = writeln!(
            out,
            "--resume: a gap whose `next_attempt_at` is in the future is deferred, not retried."
        );
    }

    let _ = writeln!(
        out,
        "\n{:>6}  {:<20} {:>8} {:>8} {:>8}",
        "wanted", "kind", "missing", "held", "queued"
    );
    let _ = writeln!(
        out,
        "{:>6}  {:<20} {:>8} {:>8} {:>8}",
        "", plan.kind, plan.missing_total, plan.held_total, plan.queued_total
    );
    if plan.held_at_another_version > 0 {
        let _ = writeln!(
            out,
            "\n{} of the {} missing are works we DO hold a full text for, at a version the want \
             does not accept (ADR-007 §2 `wrong_version`).",
            plan.held_at_another_version, plan.missing_total
        );
    }
    if plan.human_action_total > 0 {
        let _ = writeln!(
            out,
            "{} missing entr{} waiting on a person, not on a fetch — see `scitadel action_list`.",
            plan.human_action_total,
            if plan.human_action_total == 1 {
                "y is"
            } else {
                "ies are"
            }
        );
    }
    if !plan.unmatched.is_empty() {
        let _ = writeln!(
            out,
            "Named but not acquired: {} — no recorded want of this kind for {}.",
            plan.unmatched.join(", "),
            if plan.unmatched.len() == 1 {
                "it"
            } else {
                "them"
            }
        );
    }

    if plan.works.is_empty() {
        let _ = writeln!(
            out,
            "\nNothing to fetch: every wanted {} is either held or waiting on a person.",
            plan.kind
        );
    } else {
        let verb = if plan.dry_run {
            "Would fetch"
        } else {
            "Fetching"
        };
        let _ = writeln!(out, "\n{verb} ({})", plan.works.len());
        for work in &plan.works {
            let _ = writeln!(
                out,
                "  {}  {}  [{}]",
                short_id(&work.paper_id),
                work.publisher.as_deref().unwrap_or("—"),
                work.status
            );
            if work.publisher.is_none()
                && let Some(note) = &work.publisher_note
            {
                let _ = writeln!(out, "      {note}");
            }
        }
    }
    if plan.truncated_by_limit > 0 {
        let _ = writeln!(
            out,
            "--limit {} left {} queued work(s) for a later run.",
            plan.limit.unwrap_or(0),
            plan.truncated_by_limit
        );
    }

    if !plan.deferred.is_empty() {
        let _ = writeln!(out, "\nDeferred until due ({})", plan.deferred.len());
        for work in &plan.deferred {
            let _ = writeln!(
                out,
                "  {}  [{}]  in {}s ({}), not requested",
                short_id(&work.paper_id),
                work.status,
                work.retry_in_seconds,
                work.next_attempt_at
            );
        }
    }

    if plan.dry_run {
        return out;
    }

    let _ = writeln!(
        out,
        "\nacquired {}   failed {}   deferred {}   held {} (never requested)",
        report.acquired(),
        report.failed(),
        plan.deferred.len(),
        plan.held_total
    );
    for outcome in &report.outcomes {
        match outcome.outcome {
            scitadel_adapters::acquire::FetchOutcome::Acquired => {
                let _ = writeln!(
                    out,
                    "  + {}  {} via {}  {} bytes",
                    short_id(&outcome.paper_id),
                    outcome.access.as_deref().unwrap_or(""),
                    outcome.route.as_deref().unwrap_or("?"),
                    outcome.bytes.unwrap_or(0)
                );
            }
            scitadel_adapters::acquire::FetchOutcome::Failed => {
                // The status is the walk's, and the reason enumerates the
                // routes it considered. Printing `error` without it would be
                // the report-overstates-the-evidence failure #260 was filed
                // for.
                let _ = writeln!(
                    out,
                    "  ! {}  [{}]{}",
                    short_id(&outcome.paper_id),
                    outcome.status.as_deref().unwrap_or("?"),
                    outcome
                        .next_attempt_at
                        .as_deref()
                        .map(|at| format!("  retry at {at}"))
                        .unwrap_or_default()
                );
                if let Some(reason) = &outcome.reason {
                    let _ = writeln!(out, "      {reason}");
                }
            }
            scitadel_adapters::acquire::FetchOutcome::NotAttempted => {
                let _ = writeln!(
                    out,
                    "  - {}  not attempted: {}",
                    short_id(&outcome.paper_id),
                    outcome.error.as_deref().unwrap_or("")
                );
            }
        }
    }
    out
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

/// Prompt for one credential. Secrets are read without echo when stdin is
/// a terminal; when it is a pipe there is no echo to suppress and
/// `rpassword` would fail outright on the missing tty, so a plain line
/// read keeps `scitadel auth login … < creds.txt` working.
fn prompt_credential(label: &str, secret: bool) -> Result<String> {
    print!("  {label}: ");
    std::io::stdout().flush()?;

    let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());

    if secret && tty {
        let value = rpassword::read_password().context("failed to read password")?;
        println!();
        return Ok(value.trim().to_string());
    }

    let mut value = String::new();
    std::io::stdin().read_line(&mut value)?;
    if !tty {
        // Piped input echoes nothing, so close the prompt line ourselves.
        println!();
    }
    Ok(value.trim().to_string())
}

#[cfg(test)]
mod search_output_tests {
    use super::{failed_source_names, format_outcome_line};
    use scitadel_core::models::{Search, SourceOutcome, SourceStatus};

    fn outcome(
        source: &str,
        status: SourceStatus,
        count: i32,
        error: Option<&str>,
    ) -> SourceOutcome {
        SourceOutcome {
            source: source.into(),
            status,
            result_count: count,
            latency_ms: 58.4,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn a_failed_source_reports_the_error_not_zero_results() {
        let line = format_outcome_line(&outcome(
            "openalex",
            SourceStatus::Failed,
            0,
            Some("HTTP 429 Too Many Requests — Insufficient budget."),
        ));
        assert_eq!(
            line,
            "  [!] openalex: HTTP 429 Too Many Requests — Insufficient budget. (58ms)"
        );
        assert!(
            !line.contains("0 results"),
            "a dead source must never look like an empty one: {line}"
        );
    }

    #[test]
    fn a_failed_source_without_a_message_still_reads_as_a_failure() {
        let line = format_outcome_line(&outcome("lens", SourceStatus::Failed, 0, None));
        assert_eq!(line, "  [!] lens: unknown error (58ms)");
    }

    #[test]
    fn a_genuinely_empty_source_still_reports_zero_results() {
        let line = format_outcome_line(&outcome("pubmed", SourceStatus::Success, 0, None));
        assert_eq!(line, "  [+] pubmed: 0 results (58ms)");
    }

    #[test]
    fn a_successful_source_reports_its_count() {
        let line = format_outcome_line(&outcome("arxiv", SourceStatus::Success, 12, None));
        assert_eq!(line, "  [+] arxiv: 12 results (58ms)");
    }

    #[test]
    fn failed_source_names_lists_every_non_success_status() {
        let mut search = Search::new("q");
        search.source_outcomes = vec![
            outcome("arxiv", SourceStatus::Success, 3, None),
            outcome("openalex", SourceStatus::Failed, 0, Some("HTTP 429")),
            outcome("lens", SourceStatus::Skipped, 0, None),
        ];
        assert_eq!(failed_source_names(&search), vec!["openalex", "lens"]);
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

#[cfg(test)]
mod identity_override_cli_tests {
    //! #253's escape hatch, at the surface a person actually uses.
    //!
    //! The db-side tests pin the write-once rule; these pin the two things only
    //! the CLI can show: that an override **requires** a reason, and that an
    //! override is **visible** in both projections. The second is the point of the
    //! whole flag — an override nobody can see is indistinguishable from a bug,
    //! and a work that stopped blocking because a person checked it looks exactly
    //! like one that stopped blocking because the matcher changed.

    use super::{render_action_list, render_coverage, render_identity_overrides};
    use scitadel_core::models::{Paper, PaperId};
    use scitadel_core::ports::PaperRepository as _;
    use scitadel_db::sqlite::{Database, IdentityError, IdentityOverride, StateWrite};

    const NOW: &str = "2026-01-01T00:00:00+00:00";

    fn fixture() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("scitadel.db")).unwrap();
        db.migrate().unwrap();
        let (paper_repo, ..) = db.repositories();
        let mut paper = Paper::new("Deep learning for radiopharmaceutical image reconstruction");
        paper.id = PaperId::from("p-1");
        paper_repo.save(&paper).unwrap();
        db.upsert_acquisition_state(&StateWrite {
            paper_id: "p-1".into(),
            kind: "fulltext".into(),
            locator: String::new(),
            wanted_version: "vor".into(),
            status: "identity_mismatch".into(),
            reason: Some("the served title was a different paper".into()),
            publisher: None,
            hint_url: None,
            drop_path: None,
            next_attempt_at: None,
            updated_at: NOW.into(),
        })
        .unwrap();
        (dir, db)
    }

    /// A blank reason is refused, and the work stays blocked — an override
    /// nobody explained is not a decision.
    #[test]
    fn an_override_without_a_reason_is_refused_and_changes_nothing() {
        let (_dir, db) = fixture();
        for blank in ["", "  ", "\t"] {
            assert!(
                matches!(
                    db.override_identity("p-1", blank),
                    Err(IdentityError::ReasonRequired)
                ),
                "{blank:?} must be refused"
            );
        }
        let gap = db
            .acquisition_state("p-1", "fulltext", "")
            .unwrap()
            .unwrap();
        assert_eq!(
            gap.status, "identity_mismatch",
            "the work is still blocked: nothing was settled"
        );
    }

    /// The reason a person gives is printed, with the work it applies to, and the
    /// gap is returned to the `acquire` queue — otherwise `action_list` keeps
    /// asking them to check a DOI they have just checked.
    #[test]
    fn an_override_reports_the_queue_transition_and_is_visible_in_both_projections() {
        let (_dir, db) = fixture();
        let report = db
            .override_identity("p-1", "same report; OpenAlex has the older title")
            .unwrap();
        assert_eq!(
            report.gap_status_before.as_deref(),
            Some("identity_mismatch")
        );
        assert_eq!(report.gap_status_after.as_deref(), Some("pending"));

        let rendered = super::render_override_identity(&report);
        assert!(
            rendered.contains("same report; OpenAlex has the older title"),
            "{rendered}"
        );
        assert!(
            rendered.contains("identity_mismatch -> pending"),
            "{rendered}"
        );

        let list = db.action_list().unwrap();
        let action_text = render_action_list(&list);
        let coverage_text = render_coverage(&list.coverage(None).unwrap(), list.accounted(), None);
        for (name, text) in [("action_list", &action_text), ("coverage", &coverage_text)] {
            assert!(
                text.contains("Identity overridden by a person"),
                "{name} does not show the override:\n{text}"
            );
            assert!(
                text.contains("same report; OpenAlex has the older title"),
                "{name} does not print the reason:\n{text}"
            );
        }
        // And the accounting is untouched: an override is neither a want nor an
        // artefact, so it must not move a single total.
        assert_eq!(list.report.missing_total(), 1);
        assert_eq!(list.accounted(), list.report.missing_total());
    }

    /// No overrides ⇒ no section. A library that has never been overridden must
    /// not print an empty heading on every run.
    #[test]
    fn no_overrides_prints_no_section() {
        assert!(render_identity_overrides(&[]).is_empty());
        let (_dir, db) = fixture();
        let list = db.action_list().unwrap();
        assert!(list.report.identity_overrides.is_empty());
        assert!(
            !render_action_list(&list).contains("Identity overridden"),
            "nothing has been overridden, so nothing is printed"
        );
    }

    /// The rendered shape, pinned so a future edit cannot quietly drop the
    /// heading or the reason line.
    #[test]
    fn the_override_section_names_the_work_the_phases_and_the_reason() {
        let text = render_identity_overrides(&[IdentityOverride {
            paper_id: "cafe1234-dead-beef-cafe-1234567890ab".into(),
            phases: vec![
                scitadel_db::sqlite::IdentityPhase::PreFetch,
                scitadel_db::sqlite::IdentityPhase::PostFetch,
            ],
            reason: "checked by hand against the OSTI record".into(),
            recorded_at: NOW.into(),
        }]);
        assert!(text.contains("1 work(s)"), "{text}");
        assert!(text.contains("cafe1234"), "{text}");
        assert!(text.contains("pre_fetch, post_fetch"), "{text}");
        assert!(
            text.contains("checked by hand against the OSTI record"),
            "{text}"
        );
        assert!(text.contains(NOW), "and when it was recorded: {text}");
    }
}

#[cfg(test)]
mod coverage_action_list_tests {
    //! The acceptance criterion for ADR-007 §2: "Its counts sum exactly to the
    //! missing totals in `coverage`."
    //!
    //! These tests render both commands' text from **one** `ActionList` and
    //! compare the numbers a reader would actually read off the screen, because
    //! the two projections come from one value by construction and the only
    //! thing left to prove is that the printing does not lose the count.

    use super::{ActionListJson, CoverageJson, render_action_list, render_coverage};
    use scitadel_core::models::{Paper, PaperId};
    use scitadel_core::ports::PaperRepository as _;
    use scitadel_db::sqlite::Database;
    use scitadel_db::sqlite::{
        ALL_STATUSES, ActionList, ArtefactWrite, BlobWrite, StateWrite, WriteMode,
    };

    const NOW: &str = "2026-01-01T00:00:00+00:00";

    /// A migrated database holding the named works. Everything goes through
    /// `scitadel-db`'s own API — the CLI takes no `rusqlite` dependency, and a
    /// test that reached past it would not be testing the same surface.
    fn fixture(papers: &[(&str, Option<&str>)]) -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("scitadel.db")).unwrap();
        db.migrate().unwrap();
        let (paper_repo, ..) = db.repositories();
        for (id, doi) in papers {
            let mut paper = Paper::new(format!("Work {id}"));
            paper.id = PaperId::from(*id);
            paper.doi = doi.map(str::to_string);
            paper_repo.save(&paper).unwrap();
        }
        (dir, db)
    }

    fn want(db: &Database, paper_id: &str, kind: &str, status: &str) {
        db.upsert_acquisition_state(&StateWrite {
            paper_id: paper_id.into(),
            kind: kind.into(),
            locator: String::new(),
            wanted_version: "vor".into(),
            status: status.into(),
            reason: Some(format!("why {status} happened on {paper_id}")),
            publisher: None,
            hint_url: Some("https://doi.org/10.1039/d0nr01234a".into()),
            drop_path: None,
            next_attempt_at: None,
            updated_at: NOW.into(),
        })
        .unwrap();
    }

    /// Hold a VoR full text for `paper_id`, which closes its `fulltext` want.
    fn hold_fulltext(db: &Database, paper_id: &str) {
        let sha = format!("sha-{paper_id}");
        db.write_artefacts(
            &[ArtefactWrite {
                id: String::new(),
                paper_id: paper_id.into(),
                kind: "fulltext_pdf".into(),
                version: "vor".into(),
                locator: String::new(),
                sha256: Some(sha.clone()),
                format: Some("pdf".into()),
                access_status: "full_text".into(),
                route: "publisher".into(),
                access_basis: "subscription_read".into(),
                label: None,
                caption: None,
                source_url: None,
                publisher: None,
                publisher_note: None,
                imported_from: None,
                retrieved_at: NOW.into(),
                missing_on_disk: false,
                blob: Some(BlobWrite {
                    sha256: sha,
                    bytes: 4,
                    mime: "application/pdf".into(),
                    rel_path: format!("blobs/{paper_id}.pdf"),
                    created_at: NOW.into(),
                }),
            }],
            WriteMode::Reconcile,
        )
        .unwrap();
    }

    /// A library with one work in every interesting state: a human action, a
    /// deferred status, a held full text, and a work nobody ever wanted.
    fn mixed_library() -> (tempfile::TempDir, Database) {
        let (dir, db) = fixture(&[
            ("p-ill", Some("10.1039/d0nr01234a")),
            ("p-key", Some("10.1016/j.jneumeth.2005.09.009")),
            ("p-queue", Some("10.1101/2025.06.14.659707")),
            ("p-held", Some("10.1007/s10462-023-04567-8")),
            ("p-untouched", None),
        ]);
        // Two human actions, two deferred rows, and one want a real file
        // closes — so every branch of both renderers is exercised.
        want(&db, "p-ill", "fulltext", "needs_ill");
        want(&db, "p-ill", "si", "unavailable");
        want(&db, "p-key", "fulltext", "tdm_key_missing");
        want(&db, "p-queue", "fulltext", "pending");
        want(&db, "p-held", "fulltext", "pending");
        hold_fulltext(&db, "p-held");
        (dir, db)
    }

    /// `coverage`'s two shapes and `action_list`'s text, all from one read.
    fn report_and_text(list: &ActionList, kind: Option<&str>) -> (String, String) {
        let report = list.coverage(kind).unwrap();
        let text = render_coverage(&report, list.accounted(), kind);
        let json = serde_json::to_string_pretty(&CoverageJson {
            missing_total: report.missing_total(),
            wanted_total: report.wanted_total(),
            held_total: report.held_total(),
            report: &report,
        })
        .unwrap();
        (json, text)
    }

    /// The value printed after `Missing:` — the number a reader compares with
    /// `action_list`.
    fn coverage_missing_total(text: &str) -> usize {
        text.lines()
            .find_map(|line| line.strip_prefix("Missing: "))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no `Missing: N` line in:\n{text}"))
    }

    /// The value printed after `Accounts for` — `action_list`'s own total.
    fn action_list_accounted_total(text: &str) -> usize {
        text.lines()
            .find_map(|line| line.strip_prefix("Accounts for "))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no `Accounts for N` line in:\n{text}"))
    }

    /// ADR-007 §2's acceptance criterion, asserted on the two commands' actual
    /// output: the number `coverage` prints as missing and the number
    /// `action_list` prints as accounted for are the same number.
    #[test]
    fn action_list_totals_equal_coverage_totals() {
        let (_dir, db) = mixed_library();
        let list = db.action_list().unwrap();

        let (coverage_json, coverage_text) = report_and_text(&list, None);
        let action_text = render_action_list(&list);

        let missing = coverage_missing_total(&coverage_text);
        let accounted = action_list_accounted_total(&action_text);
        assert_eq!(
            missing, accounted,
            "the two projections must print the same total\n--- coverage ---\n{coverage_text}\n\
             --- action_list ---\n{action_text}"
        );
        assert_eq!(missing, list.report.missing_total());
        assert_eq!(accounted, list.accounted());
        assert_eq!(
            missing, 4,
            "five recorded wants, one of them closed by a real file"
        );
        assert_eq!(list.human_total(), 2);
        assert_eq!(list.deferred_total(), 2);

        // And the machine-readable forms agree too, which is what CI can check.
        let json: serde_json::Value = serde_json::from_str(&coverage_json).unwrap();
        assert_eq!(json["missing_total"], serde_json::json!(missing));
        assert_eq!(
            json["by_status"]
                .as_object()
                .unwrap()
                .values()
                .map(|v| v.as_u64().unwrap())
                .sum::<u64>(),
            u64::try_from(missing).unwrap(),
            "the per-status table sums to the missing total"
        );

        let action_json = serde_json::to_string_pretty(&ActionListJson {
            missing_total: list.report.missing_total(),
            human_total: list.human_total(),
            deferred_total: list.deferred_total(),
            accounted_total: list.accounted(),
            groups: &list.groups,
            deferred: &list.deferred,
            report: &list.report,
        })
        .unwrap();
        let action_json: serde_json::Value = serde_json::from_str(&action_json).unwrap();
        assert_eq!(
            action_json["accounted_total"], action_json["missing_total"],
            "the JSON carries both sides of the criterion"
        );
        assert_eq!(
            action_json["human_total"].as_u64().unwrap()
                + action_json["deferred_total"].as_u64().unwrap(),
            action_json["accounted_total"].as_u64().unwrap()
        );

        // Both projections see the same entries, not merely the same number.
        let grouped: Vec<&scitadel_db::sqlite::MissingEntry> =
            list.groups.iter().flat_map(|g| g.entries.iter()).collect();
        let deferred: usize = list.deferred.iter().map(|d| d.count).sum();
        assert_eq!(grouped.len() + deferred, missing);
        for entry in &list.report.entries {
            let in_a_group = grouped.contains(&entry);
            let in_deferred = matches!(
                entry.status.as_str(),
                "pending"
                    | "oa_fetchable"
                    | "tdm_available"
                    | "unavailable"
                    | "rate_limited"
                    | "error"
            );
            assert!(
                in_a_group ^ in_deferred,
                "{entry:?} must be on exactly one side of the partition"
            );
        }
    }

    /// #261, end to end: an unclassified registrant prefix must not produce a
    /// publisher name and must not be reported as lacking a TDM route, in
    /// either command's output.
    #[test]
    fn an_unclassified_publisher_prints_no_route_verdict() {
        let (_dir, db) = fixture(&[("p-odd", Some("10.99999/some.suffix.12345"))]);
        want(&db, "p-odd", "fulltext", "needs_ill");
        let list = db.action_list().unwrap();

        let (coverage_json, coverage_text) = report_and_text(&list, None);
        let action_text = render_action_list(&list);

        for (name, text) in [
            ("coverage", coverage_text.as_str()),
            ("action_list", action_text.as_str()),
            ("coverage --json", coverage_json.as_str()),
        ] {
            assert!(
                !text.contains("no TDM route available"),
                "{name} reported a TDM verdict for a publisher nobody classified:\n{text}"
            );
            for publisher in scitadel_core::publisher::ALL_PUBLISHERS {
                assert!(
                    !text.contains(publisher.display_name()),
                    "{name} named an unclassified publisher ({}) :\n{text}",
                    publisher.display_name()
                );
            }
        }
        // What it *does* say is the registry's own wording, prefix included.
        for text in [&coverage_text, &action_text] {
            assert!(text.contains("not classified"), "{text}");
            assert!(text.contains("99999"), "{text}");
            assert!(text.contains("undetermined"), "{text}");
        }
    }

    /// `pending` belongs to the `acquire` queue, not to a person — and the
    /// report says so out loud rather than dropping it, because dropping it
    /// would make `action_list` under-count `coverage`.
    #[test]
    fn a_pending_work_is_not_in_the_human_action_list() {
        let (_dir, db) = fixture(&[
            ("p-pending", Some("10.1101/2025.06.14.659707")),
            ("p-ill", Some("10.1039/d0nr01234a")),
        ]);
        want(&db, "p-pending", "fulltext", "pending");
        want(&db, "p-ill", "fulltext", "needs_ill");

        let list = db.action_list().unwrap();
        let action_text = render_action_list(&list);
        let (_, coverage_text) = report_and_text(&list, None);

        // Not in a group.
        assert_eq!(list.groups.len(), 1);
        assert_eq!(list.groups[0].status, "needs_ill");
        assert!(
            list.groups[0]
                .entries
                .iter()
                .all(|e| e.paper_id != "p-pending"),
            "{:?}",
            list.groups[0].entries
        );
        // Still counted, and named where it went.
        assert_eq!(list.human_total(), 1);
        assert_eq!(list.deferred_total(), 1);
        let deferred = list
            .deferred
            .iter()
            .find(|d| d.status == "pending")
            .expect("pending is accounted for");
        assert_eq!(deferred.count, 1);
        assert!(deferred.goes_to.contains("acquire"), "{}", deferred.goes_to);

        assert!(action_text.contains("Not a human action"), "{action_text}");
        assert!(action_text.contains("never tried"), "{action_text}");
        assert!(
            coverage_missing_total(&coverage_text) == 2,
            "coverage still counts it: {coverage_text}"
        );
        assert_eq!(
            action_list_accounted_total(&action_text),
            coverage_missing_total(&coverage_text)
        );
    }

    /// Every status is printed even at zero, so a reader can tell "no such
    /// papers" from "this category is not tracked".
    #[test]
    fn the_text_output_names_every_status_and_kind() {
        let (_dir, db) = fixture(&[("p-one", Some("10.1039/d0nr01234a"))]);
        want(&db, "p-one", "fulltext", "needs_ill");
        let list = db.action_list().unwrap();
        let (_, text) = report_and_text(&list, None);
        for vocab in ALL_STATUSES {
            assert!(
                text.contains(vocab.status),
                "{} does not appear in the coverage output:\n{text}",
                vocab.status
            );
        }
        for kind in scitadel_db::sqlite::ALL_WANT_KINDS {
            assert!(text.contains(kind), "{kind} missing from:\n{text}");
        }
    }

    /// `--kind` narrows every number it prints, and the scope counters say
    /// they are not narrowed rather than quietly reporting a filtered number.
    #[test]
    fn the_kind_filter_narrows_the_printed_numbers() {
        let (_dir, db) = mixed_library();
        let list = db.action_list().unwrap();
        let (_, text) = report_and_text(&list, Some("si"));
        assert!(text.contains("Restricted to --kind si"), "{text}");
        assert_eq!(coverage_missing_total(&text), 1, "{text}");
        // The library-wide line survives the filter and says so.
        assert!(text.contains("library-wide"), "{text}");
        assert!(text.contains("works in library"), "{text}");
    }

    /// A golden-ish smoke test of the human output: it is the only place the
    /// two projections are seen side by side, so a formatting change that made
    /// a number unreadable would show up here.
    #[test]
    fn the_rendered_reports_read_the_way_they_are_meant_to() {
        let (_dir, db) = mixed_library();
        let list = db.action_list().unwrap();
        let (_, coverage_text) = report_and_text(&list, None);
        let action_text = render_action_list(&list);
        println!("=== scitadel coverage ===\n{coverage_text}");
        println!("=== scitadel action_list ===\n{action_text}");
        assert!(coverage_text.contains("Royal Society of Chemistry has no TDM route available"));
        assert!(action_text.contains("register a key — elsevier"));
    }

    /// A work nobody wanted is reported, never counted.
    #[test]
    fn an_unwanted_work_is_reported_and_not_counted() {
        let (_dir, db) = fixture(&[("p-quiet", Some("10.1039/d0nr01234a"))]);
        let list = db.action_list().unwrap();
        let (json, text) = report_and_text(&list, None);
        assert_eq!(coverage_missing_total(&text), 0, "{text}");
        assert!(text.contains("works untracked"), "{text}");
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["untracked_works"], serde_json::json!(1));
        assert_eq!(json["works_in_scope"], serde_json::json!(0));
        assert_eq!(json["missing_total"], serde_json::json!(0));
    }
}
