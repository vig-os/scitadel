use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;

#[derive(Parser)]
#[command(
    name = "scitadel",
    version,
    about = "Programmable, reproducible scientific literature retrieval"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize scitadel: write a config and create the database.
    /// Runs as an interactive wizard unless --yes or stdin is non-interactive.
    Init {
        /// Database path
        #[arg(long)]
        db: Option<PathBuf>,
        /// OpenAlex / Unpaywall email (used for OA PDF lookups)
        #[arg(long)]
        email: Option<String>,
        /// Comma-separated sources to enable (e.g. pubmed,arxiv,openalex)
        #[arg(long)]
        sources: Option<String>,
        /// Non-interactive: use provided flags + defaults, never prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Run a federated literature search
    Search {
        /// Search query
        query: Option<String>,
        /// Comma-separated list of sources
        #[arg(short, long, default_value = "pubmed,arxiv,openalex,inspire")]
        sources: String,
        /// Maximum results per source
        #[arg(short = 'n', long, default_value = "50")]
        max_results: usize,
        /// Research question ID — auto-builds query from linked terms
        #[arg(short, long)]
        question: Option<String>,
    },
    /// Show past search runs
    History {
        /// Number of recent searches
        #[arg(short = 'n', long, default_value = "20")]
        limit: i64,
    },
    /// Show paper details
    Show {
        /// Paper or search ID
        id: String,
    },
    /// Export search results
    Export {
        /// Search ID
        search_id: String,
        /// Export format
        #[arg(short, long, default_value = "json", value_parser = ["bibtex", "json", "csv"])]
        format: String,
        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Diff two search runs
    Diff {
        /// First search ID
        search_a: String,
        /// Second search ID
        search_b: String,
    },
    /// Manage research questions
    Question {
        #[command(subcommand)]
        command: QuestionCommands,
    },
    /// Score papers against a research question using Claude
    Assess {
        /// Search ID
        search_id: String,
        /// Research question ID
        #[arg(short, long)]
        question: String,
        /// Model for scoring
        #[arg(short, long, default_value = "claude-sonnet-4-6")]
        model: String,
        /// Temperature for scoring
        #[arg(short, long, default_value = "0.0")]
        temperature: f64,
        /// Scorer backend: auto, cli, api
        #[arg(long, default_value = "auto")]
        scorer: String,
    },
    /// Download a paper (PDF or HTML) by DOI
    Download {
        /// DOI of the paper to download
        doi: String,
        /// Output directory (default: .scitadel/papers/)
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
    },
    /// Manage source credentials (keychain storage)
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
    /// Launch MCP server on stdio
    Mcp,
    /// Launch interactive TUI dashboard
    Tui {
        /// Override the active theme for this session (#137).
        /// Accepts: `auto` | `dark` | `light` | `dalton-dark` | `dalton-bright`.
        /// Precedence: this flag > `SCITADEL_THEME` env > config > auto.
        #[arg(long)]
        theme: Option<String>,
    },
    /// Bibliographic operations: import / export / rekey / watch (#134)
    Bib {
        #[command(subcommand)]
        command: BibCommands,
    },
    /// Run citation chaining (snowballing)
    Snowball {
        /// Search ID
        search_id: String,
        /// Research question ID
        #[arg(short, long)]
        question: String,
        /// Max chaining depth (1-3)
        #[arg(short, long, default_value = "1")]
        depth: i32,
        /// Min relevance score to expand
        #[arg(long, default_value = "0.6")]
        threshold: f64,
        /// Citation direction
        #[arg(long, default_value = "both", value_parser = ["references", "cited_by", "both"])]
        direction: String,
        /// Model for scoring
        #[arg(short, long, default_value = "claude-sonnet-4-6")]
        model: String,
    },
}

#[derive(Subcommand)]
enum BibCommands {
    /// Import a `.bib` file (BibTeX or BibLaTeX), matching entries
    /// against existing papers via DOI/arXiv/PubMed/OpenAlex/citekey/
    /// title+year. Imported citekeys are recorded as aliases so
    /// re-importing is a no-op.
    Import {
        /// Path to the .bib file (Zotero / Mendeley export)
        path: PathBuf,
        /// Merge strategy: reject | db-wins | bib-wins | merge
        #[arg(long, default_value = "merge")]
        strategy: String,
        /// Identity attached to imported annotations (`note=` field).
        /// Defaults to $USER.
        #[arg(long)]
        reader: Option<String>,
        /// Show per-row trace output (dropped files, dropped keywords).
        #[arg(short, long)]
        verbose: bool,
    },
    /// Reassign a paper's citation key. Without `--key`, re-runs the
    /// #132 algorithm against current paper metadata; with `--key`,
    /// sets an explicit key. Old key is preserved as an alias so
    /// manuscripts that still cite by the old key keep resolving.
    /// Fails loudly on collision with another paper's existing key.
    Rekey {
        /// Paper id to rekey (full id or unambiguous prefix).
        paper_id: String,
        /// Explicit citation key. If omitted, the algorithm picks one.
        #[arg(long)]
        key: Option<String>,
        /// Identity recorded in the audit log. Defaults to $USER.
        #[arg(long)]
        reader: Option<String>,
    },
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Store credentials for a source in the system keychain
    Login {
        /// Source name (pubmed, openalex, lens, epo)
        source: String,
    },
    /// Remove stored credentials for a source
    Logout {
        /// Source name
        source: String,
    },
    /// Show which sources have credentials configured
    Status,
}

#[derive(Subcommand)]
enum QuestionCommands {
    /// Create a research question
    Create {
        /// Question text
        text: String,
        /// Additional context
        #[arg(short, long, default_value = "")]
        description: String,
    },
    /// List all research questions
    List,
    /// Add search terms linked to a question
    AddTerms {
        /// Question ID
        question_id: String,
        /// Search terms
        #[arg(required = true)]
        terms: Vec<String>,
        /// Custom query string
        #[arg(short, long)]
        query: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init {
            db,
            email,
            sources,
            yes,
        } => commands::init(commands::InitOptions {
            db_path: db,
            email,
            sources: sources.map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            }),
            yes,
        }),
        Commands::Search {
            query,
            sources,
            max_results,
            question,
        } => commands::search(query, sources, max_results, question).await,
        Commands::History { limit } => commands::history(limit),
        Commands::Show { id } => commands::show(&id),
        Commands::Export {
            search_id,
            format,
            output,
        } => commands::export(&search_id, &format, output),
        Commands::Diff { search_a, search_b } => commands::diff(&search_a, &search_b),
        Commands::Question { command } => match command {
            QuestionCommands::Create { text, description } => {
                commands::question_create(&text, &description)
            }
            QuestionCommands::List => commands::question_list(),
            QuestionCommands::AddTerms {
                question_id,
                terms,
                query,
            } => commands::question_add_terms(&question_id, &terms, query),
        },
        Commands::Auth { command } => match command {
            AuthCommands::Login { source } => commands::auth_login(&source),
            AuthCommands::Logout { source } => commands::auth_logout(&source),
            AuthCommands::Status => commands::auth_status(),
        },
        Commands::Download { doi, output_dir } => commands::download(&doi, output_dir).await,
        Commands::Mcp => commands::mcp().await,
        Commands::Tui { theme } => commands::tui(theme.as_deref()),
        Commands::Assess {
            search_id,
            question,
            model,
            temperature,
            scorer,
        } => commands::assess(&search_id, &question, &model, temperature, &scorer).await,
        Commands::Bib { command } => match command {
            BibCommands::Import {
                path,
                strategy,
                reader,
                verbose,
            } => commands::bib_import(&path, &strategy, reader, verbose),
            BibCommands::Rekey {
                paper_id,
                key,
                reader,
            } => commands::bib_rekey(&paper_id, key.as_deref(), reader),
        },
        Commands::Snowball {
            search_id,
            question,
            depth,
            threshold,
            direction,
            model,
        } => commands::snowball(&search_id, &question, depth, threshold, &direction, &model),
    }
}
