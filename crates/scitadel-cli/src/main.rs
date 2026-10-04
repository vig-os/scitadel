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
        /// OpenAlex relevance mode: `any` (broad fulltext — default,
        /// pre-#210 behaviour), `title` (`filter=title.search:` — best
        /// for exact titles), `auto` (title first, broad fills tail).
        /// Non-OpenAlex sources ignore this flag.
        #[arg(long, default_value = "any", value_parser = ["any", "title", "auto"])]
        field: String,
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
    /// Resolve a DOI to full metadata via OpenAlex (#210).
    ///
    /// Prints the resolved paper (title, authors, year, journal, DOI,
    /// OpenAlex id) and, unless `--no-save`, persists it in the DB so
    /// downstream `show` / `download` / `assess` commands can address
    /// it by id. Errors on a malformed DOI without hitting the wire;
    /// exits with a clear "not found" message on a valid DOI OpenAlex
    /// doesn't know about.
    ResolveDoi {
        /// DOI to resolve (bare `10.…/…` or `https://doi.org/…`)
        doi: String,
        /// Print the raw OpenAlex JSON envelope instead of the summary
        #[arg(long)]
        json: bool,
        /// Do not persist the resolved paper to the DB (print only)
        #[arg(long)]
        no_save: bool,
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
        /// Print the registered themes (with one-line descriptions) and
        /// exit without launching the TUI (#137).
        #[arg(long, conflicts_with = "theme")]
        list_themes: bool,
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
        #[arg(long, default_value = "1")]
        depth: i32,
        /// Min relevance score to expand
        #[arg(long, default_value = "0.6")]
        threshold: f64,
        /// Citation direction
        #[arg(long, default_value = "both", value_parser = ["references", "cited_by", "both"])]
        direction: String,
        /// Model for scoring
        #[arg(long, default_value = "claude-sonnet-4-6")]
        model: String,
    },
    /// Import a flat / legacy directory tree of files as `artefacts`
    /// (ADR-007 §1 "Legacy data"). Reads the layout raid and older
    /// scitadel versions wrote — `fulltext.pdf`, `si/`, `tables/`,
    /// `figures/` — under a work's directory, copies each file into the
    /// content-addressed blob store, and records where every file came
    /// from. Idempotent: re-running an unchanged tree writes nothing.
    ///
    /// `--root` accepts either the work's own directory or a library
    /// root containing `papers/<paper-stem>/`.
    ImportFlat {
        /// Paper id (full id or unambiguous prefix)
        #[arg(long)]
        paper: String,
        /// The flat-layout directory to import
        #[arg(long)]
        root: PathBuf,
    },
    /// What the library wants and does not hold (ADR-007 §1, §2)
    ///
    /// Per-kind and per-status totals plus every missing entry. "Have" is
    /// derived from `artefacts` on read — there is no stored have-status — and a
    /// work with no recorded want is reported as untracked rather than counted
    /// as missing. Every status the schema allows appears, including at zero,
    /// so "no such papers" is distinguishable from "not tracked".
    Coverage {
        /// Restrict to one artefact kind (fulltext, fulltext_pdf,
        /// fulltext_html, fulltext_xml, si, table, figure)
        #[arg(long, value_parser = scitadel_db::sqlite::ALL_WANT_KINDS)]
        kind: Option<String>,
        /// Emit the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// One grouped list of what still needs a person (ADR-007 §2)
    ///
    /// The missing entries grouped by (action group, publisher), carrying each
    /// entry's `hint_url` and `drop_path`. Its counts equal the `coverage`
    /// missing totals exactly: every missing entry is either in a group or
    /// listed below with the reason it is not a human action. A publisher
    /// scitadel could not classify is never named, and never reported as
    /// lacking a TDM route.
    ///
    /// Named `action_list` because ADR-007 §2 and issue #253 both spell it that
    /// way, and MCP's read-only verb is `action_list` too; `action-list` works
    /// as well, for anyone who expects the workspace's kebab-case spelling.
    #[command(name = "action_list", visible_alias = "action-list")]
    ActionList {
        /// Emit the grouped list as JSON
        #[arg(long)]
        json: bool,
    },
    /// Settle a work's identity by hand: the identity gate stops blocking it
    /// (#253)
    ///
    /// ADR-007 §3 says a mismatch "blocks filing anything under that DOI", and
    /// every other verdict scitadel records is chosen so a person can overturn
    /// it. This is that door. It records an `overridden` row against **both**
    /// identity phases with a reason you have to type — an unexplained override
    /// is indistinguishable from a bug — and returns a work blocked by a
    /// mismatch to the `acquire` queue, because leaving it there would keep
    /// asking you to check a DOI you have just checked.
    ///
    /// A later `acquire` run does not re-ask the question and does not undo this:
    /// an `overridden` row is write-once against machine verdicts. The override is
    /// printed by `scitadel coverage` and `scitadel action_list`.
    ///
    /// A separate command rather than a flag on `acquire`, because a flag that
    /// silently also drained the queue would make "check this one DOI" and "fetch
    /// the whole library" the same invocation.
    OverrideIdentity {
        /// Paper id (full id or unambiguous prefix)
        paper: String,
        /// Why the machine was wrong. Required — there is no default, and an empty
        /// reason is refused.
        #[arg(long, value_name = "TEXT")]
        reason: String,
        /// Emit the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Fetch the full texts the library wants and does not hold (ADR-007 §2, §3)
    ///
    /// Drains the `acquire` queue — the recorded gaps ADR-007 §2 routes to
    /// `acquire` — through the existing download ladder. Whether a work is
    /// already held is decided from `artefacts` **before** anything is
    /// fetched, so a second run makes no network call for a file the library
    /// already has.
    ///
    /// Statuses that need a person (`needs_ill`, `needs_login`,
    /// `not_entitled`, `identity_mismatch`, `wrong_version`, …) are not
    /// fetched: `scitadel action_list` owns those. A work the library already
    /// holds is not fetched either, and is reported as held rather than as
    /// nothing to do.
    ///
    /// `--kind` accepts `fulltext` only: every ADR-007 §3 route is a
    /// full-text route, so no other want can be closed by a fetch.
    ///
    /// `--from <works.ndjson>` imports raid's `p1` / `p3` queue first, so an
    /// imported gap becomes actionable by this very command. Raid's own
    /// curation statuses are **preserved verbatim** — scitadel's
    /// `acquisition_state` is about wants and gaps, and raid is the authority on
    /// curation — so a curation status scitadel has no word for is kept
    /// character-for-character in the gap's `reason` and listed in the report,
    /// never translated into a scitadel status that reads like it.
    ///
    /// `--from` writes, so it is refused under `--dry-run`: the flag promises no
    /// writes.
    Acquire {
        /// Import a raid NDJSON acquisition queue before draining it
        #[arg(long, value_name = "FILE")]
        from: Option<PathBuf>,
        /// Print the plan — what would be fetched, what is held, what is
        /// deferred — and write nothing and request nothing
        #[arg(long)]
        dry_run: bool,
        /// Only fetch what is due: a gap whose `next_attempt_at` is in the
        /// future is deferred to a later run instead of retried
        #[arg(long)]
        resume: bool,
        /// Restrict to one want kind. Only `fulltext` is acquirable.
        #[arg(long, value_parser = ["fulltext"])]
        kind: Option<String>,
        /// Stop after this many works (applied to the works that are due)
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// Emit the plan and the per-work outcomes as JSON
        #[arg(long)]
        json: bool,
    },

    /// Reconcile manually dropped-in files against the database (ADR-007 §1)
    ///
    /// The one command that does this, by design: ADR-007 §1 says manual
    /// drop-ins "are reconciled only by an explicit `scitadel scan` or
    /// `scitadel attach`, never as a side effect of reading", so reading a work
    /// never files a file that happens to be lying next to it.
    ///
    /// Walks the work's own directory (`<library>/papers/<stem>/`) unless
    /// `--root` names another, checks every file's **magic bytes against its
    /// declared format** and its size against ADR-007 §1's caps, takes the
    /// ADR-007 §3 post-fetch identity check on every supplementary file, flags a
    /// recorded file that has disappeared as `missing_on_disk = 1` (keeping the
    /// row), and regenerates the work's `manifest.json` — written by the lease
    /// holder, after the database commit, through a rename.
    ///
    /// Idempotent: an unchanged directory writes nothing. `--dry-run` reports
    /// every decision without writing a row, without copying a blob and without
    /// taking the work's lease.
    Scan {
        /// Paper id (full id or unambiguous prefix)
        #[arg(long)]
        paper: String,
        /// The directory to reconcile. Defaults to the work's own
        /// `papers/<stem>/` directory.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Report every decision — including the refusals — and write nothing
        #[arg(long)]
        dry_run: bool,
        /// Emit the report as JSON
        #[arg(long)]
        json: bool,
    },

    /// File one manually dropped-in file under a work (ADR-007 §1)
    ///
    /// For the case where you know exactly which work a file belongs to. Same
    /// checks as `scitadel scan` — magic bytes against the declared format,
    /// ADR-007 §1's size cap, the post-fetch identity check, and a refused file
    /// records an `acquisition_attempts` row (`bad_magic`, `too_large` or
    /// `identity_mismatch`) with **no artefact created**.
    ///
    /// `--kind` must match the file: `fulltext` resolves through the extension,
    /// and naming `fulltext_pdf` for an HTML file is refused rather than filed
    /// under a format its bytes do not have.
    Attach {
        /// Paper id (full id or unambiguous prefix)
        paper: String,
        /// The file to file
        file: PathBuf,
        /// Which slot it fills
        #[arg(long, value_parser = scitadel_adapters::scan::ATTACH_KINDS)]
        kind: String,
        /// The human name, e.g. "Supporting Information S1". The stable locator
        /// is derived from it unless `--locator` is given.
        #[arg(long, value_name = "TEXT")]
        label: Option<String>,
        /// The stable locator, if it is not derived from `--label`
        #[arg(long, value_name = "TEXT")]
        locator: Option<String>,
        /// Emit the report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Collect blobs no artefact references (ADR-007 §1 "Artefact rules")
    ///
    /// "Unreferenced blobs are collected by `scitadel gc`." A blob is referenced
    /// exactly when some `artefacts` row names it, and every deletion is
    /// re-checked under the write lock immediately before it happens.
    ///
    /// Conservative by construction, because a wrong deletion destroys the only
    /// copy of a file:
    ///
    /// - `--dry-run` (the default shape of the flag) reports what *would* be
    ///   collected and removes nothing;
    /// - a blob referenced by **any** artefact is never deleted, and so is a blob
    ///   younger than `--min-age-hours` (24 by default), which is what keeps an
    ///   in-flight fetch's staged bytes alive;
    /// - only a path that is a blob path **for its own digest** is ever removed —
    ///   `blobs/<2 hex>/<digest>.<ext>`, relative to the library root;
    /// - `blobs/.tmp/` is never a candidate;
    /// - unreferenced files no `blobs` row names are **counted, never deleted**.
    Gc {
        /// Report what would be collected and delete nothing
        #[arg(long)]
        dry_run: bool,
        /// How old a blob must be before it is collected (hours). Widening this
        /// is the safe direction; lowering it to 0 is only right when no other
        /// process is using the library.
        #[arg(long, value_name = "HOURS", default_value_t = scitadel_db::sqlite::DEFAULT_MIN_AGE_HOURS)]
        min_age_hours: i64,
        /// Emit the report as JSON
        #[arg(long)]
        json: bool,
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
        /// Merge strategy: reject | db-wins | bib-wins | merge | interactive
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
    /// One-shot deterministic snapshot of a question's shortlist to a
    /// `.bib` (or `.json` with `--format csl-json`) plus
    /// `.scitadel-bib.lock` sidecar (#178, #135). Same shortlist twice ⇒
    /// byte-identical output (sidecar `generated_at` excepted). Pair
    /// with `bib verify` in CI to catch drift.
    Snapshot {
        /// Research question id (full or unambiguous prefix).
        question_id: String,
        /// Output path. Defaults to `paper.bib` for `--format bibtex`
        /// and `paper.json` for `--format csl-json`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Reader scope for the shortlist. Defaults to $USER.
        #[arg(long)]
        reader: Option<String>,
        /// Skip writing the sidecar (CI one-offs). The export is still
        /// emitted but `bib verify` will exit 2 with "no lockfile".
        #[arg(long)]
        no_lock: bool,
        /// Output flavor: `bibtex` (BibLaTeX, default) or `csl-json`
        /// (CSL 1.0.2 canonical). Sidecar's `format` discriminant
        /// records the choice so verify routes to the matching emitter.
        #[arg(long, default_value = "bibtex", value_parser = ["bibtex", "csl-json"])]
        format: String,
    },
    /// Verify a committed export against its `.scitadel-bib.lock`
    /// (#178). Routes to BibTeX or CSL-JSON based on the sidecar's
    /// `format` field. Exit codes: `0` ok, `1` drift (regenerate to fix),
    /// `2` stale (binary moved or sidecar absent — regenerate required).
    Verify {
        /// Path to the export to verify.
        file: PathBuf,
        /// Override the sidecar's question_id (rarely needed).
        #[arg(short, long)]
        question_id: Option<String>,
        /// Output format for the verify report: `text` (default) or
        /// `json` (CI-friendly). Independent of the snapshot's flavor —
        /// that's read from the sidecar.
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// Structural diff between two bibliography exports — entry-level
    /// (added / removed / changed) rather than line-level. Mirrors the
    /// human-readable "what actually moved" report `bib verify` only
    /// hints at via its line diff. Auto-detects BibTeX vs CSL-JSON by
    /// content sniff; the two flavors are interchangeable. Exit codes:
    /// `0` no diff, `1` diff (mirrors `git diff`).
    Diff {
        /// First file (BibTeX or CSL-JSON).
        file_a: PathBuf,
        /// Second file. Mutually exclusive with `--question-id`.
        file_b: Option<PathBuf>,
        /// Compare `file_a` against a fresh snapshot of this question's
        /// shortlist from the DB. Mutually exclusive with `<file_b>`.
        #[arg(long)]
        question_id: Option<String>,
        /// Output format: `text` (default, hand-rolled ANSI when
        /// stdout is a TTY) or `json` (structured, CI-friendly).
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
        /// Force ANSI color off (useful for piping into `less` without
        /// `-R`). Color is also auto-disabled when stdout is not a TTY.
        #[arg(long)]
        no_color: bool,
        /// Reader scope when comparing against `--question-id`. Defaults
        /// to $USER. Ignored when `<file_b>` is provided.
        #[arg(long)]
        reader: Option<String>,
    },
    /// Live `.bib` snapshot for a research question's shortlist.
    /// Polls SQLite at 1s, debounces bursts of edits, and skips
    /// the file write when the rendered content is byte-identical
    /// (hash-and-skip) so unrelated DB churn doesn't thrash the
    /// output. SIGINT/SIGTERM flushes any pending change before exit.
    Watch {
        /// Research question id (full or unambiguous prefix).
        question_id: String,
        /// Output `.bib` path. Will be (re)written each time the
        /// shortlist's content changes.
        #[arg(short, long)]
        output: PathBuf,
        /// Reader scope for the shortlist. Defaults to $USER.
        #[arg(long)]
        reader: Option<String>,
        /// Drop papers whose max assessment score for this question
        /// is below this threshold. Default: include everything.
        #[arg(long)]
        min_score: Option<f64>,
        /// Debounce window in milliseconds — bursts of edits within
        /// this window coalesce into a single write.
        #[arg(long, default_value = "300")]
        debounce_ms: u64,
        /// Polling interval in milliseconds. Lower = lower latency
        /// but more SQLite reads.
        #[arg(long, default_value = "1000")]
        poll_ms: u64,
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
            field,
        } => commands::search(query, sources, max_results, question, &field).await,
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
        Commands::ResolveDoi { doi, json, no_save } => {
            commands::resolve_doi(&doi, json, no_save).await
        }
        Commands::Mcp => commands::mcp().await,
        Commands::Tui { theme, list_themes } => {
            if list_themes {
                commands::list_themes()
            } else {
                commands::tui(theme.as_deref())
            }
        }
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
            BibCommands::Snapshot {
                question_id,
                output,
                reader,
                no_lock,
                format,
            } => commands::bib_snapshot(
                &question_id,
                output.as_deref(),
                reader.as_deref(),
                no_lock,
                &format,
            ),
            BibCommands::Verify {
                file,
                question_id,
                format,
            } => {
                let code = commands::bib_verify(&file, question_id.as_deref(), &format)?;
                std::process::exit(code);
            }
            BibCommands::Diff {
                file_a,
                file_b,
                question_id,
                format,
                no_color,
                reader,
            } => {
                let code = commands::bib_diff(
                    &file_a,
                    file_b.as_deref(),
                    question_id.as_deref(),
                    &format,
                    no_color,
                    reader.as_deref(),
                )?;
                std::process::exit(code);
            }
            BibCommands::Watch {
                question_id,
                output,
                reader,
                min_score,
                debounce_ms,
                poll_ms,
            } => {
                commands::bib_watch(
                    &question_id,
                    &output,
                    reader,
                    min_score,
                    debounce_ms,
                    poll_ms,
                )
                .await
            }
        },
        Commands::Snowball {
            search_id,
            question,
            depth,
            threshold,
            direction,
            model,
        } => commands::snowball(&search_id, &question, depth, threshold, &direction, &model),
        Commands::ImportFlat { paper, root } => commands::import_flat(&paper, &root),
        Commands::Coverage { kind, json } => commands::coverage(kind.as_deref(), json),
        Commands::ActionList { json } => commands::action_list(json),
        Commands::OverrideIdentity {
            paper,
            reason,
            json,
        } => commands::override_identity(&paper, &reason, json),
        Commands::Acquire {
            from,
            dry_run,
            resume,
            kind,
            limit,
            json,
        } => {
            commands::acquire(
                commands::AcquireOptions {
                    from,
                    dry_run,
                    resume,
                    kind,
                    limit,
                },
                json,
            )
            .await
        }
        Commands::Scan {
            paper,
            root,
            dry_run,
            json,
        } => commands::scan(&paper, root.as_deref(), dry_run, json),
        Commands::Attach {
            paper,
            file,
            kind,
            label,
            locator,
            json,
        } => commands::attach(
            &paper,
            &file,
            &kind,
            label.as_deref(),
            locator.as_deref(),
            json,
        ),
        Commands::Gc {
            dry_run,
            min_age_hours,
            json,
        } => commands::gc(dry_run, min_age_hours, json),
    }
}
