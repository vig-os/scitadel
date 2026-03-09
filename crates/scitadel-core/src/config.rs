use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Per-source adapter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timeout")]
    pub timeout: f64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub api_key: String,
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: 30.0,
            max_retries: 3,
            api_key: String::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> f64 {
    30.0
}

fn default_max_retries() -> u32 {
    3
}

/// Configuration for Claude-based scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_scoring_concurrency")]
    pub scoring_concurrency: u32,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            scoring_concurrency: 5,
        }
    }
}

fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_scoring_concurrency() -> u32 {
    5
}

/// Top-level application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub db_path: PathBuf,
    #[serde(default = "default_sources")]
    pub default_sources: Vec<String>,
    #[serde(default)]
    pub pubmed: SourceConfig,
    #[serde(default)]
    pub arxiv: SourceConfig,
    #[serde(default)]
    pub openalex: SourceConfig,
    #[serde(default)]
    pub inspire: SourceConfig,
    #[serde(default)]
    pub chat: ChatConfig,
}

fn default_sources() -> Vec<String> {
    vec![
        "pubmed".into(),
        "arxiv".into(),
        "openalex".into(),
        "inspire".into(),
    ]
}

impl Default for Config {
    fn default() -> Self {
        let workspace =
            find_workspace_root().unwrap_or_else(|| std::env::current_dir().unwrap());
        Self {
            db_path: default_db_path(&workspace),
            default_sources: default_sources(),
            pubmed: SourceConfig::default(),
            arxiv: SourceConfig::default(),
            openalex: SourceConfig::default(),
            inspire: SourceConfig::default(),
            chat: ChatConfig::default(),
        }
    }
}

/// Find the workspace root by looking for a git repo from cwd upward.
fn find_workspace_root() -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if output.status.success() {
        let path = String::from_utf8(output.stdout).ok()?;
        Some(PathBuf::from(path.trim()))
    } else {
        None
    }
}

/// Resolve the default database path.
///
/// Priority: `SCITADEL_DB` env var > workspace `.scitadel/scitadel.db` > cwd.
fn default_db_path(workspace: &Path) -> PathBuf {
    if let Ok(db) = std::env::var("SCITADEL_DB") {
        let expanded = if db.starts_with('~') {
            if let Ok(home) = std::env::var("HOME") {
                db.replacen('~', &home, 1)
            } else {
                db
            }
        } else {
            db
        };
        return PathBuf::from(expanded);
    }
    workspace.join(".scitadel").join("scitadel.db")
}

/// Load configuration from environment variables and optional TOML file.
pub fn load_config() -> Config {
    let workspace =
        find_workspace_root().unwrap_or_else(|| std::env::current_dir().unwrap());
    let db_path = default_db_path(&workspace);

    let pubmed = SourceConfig {
        api_key: std::env::var("SCITADEL_PUBMED_API_KEY").unwrap_or_default(),
        ..Default::default()
    };

    let openalex = SourceConfig {
        api_key: std::env::var("SCITADEL_OPENALEX_EMAIL").unwrap_or_default(),
        ..Default::default()
    };

    let chat = ChatConfig {
        model: std::env::var("SCITADEL_CHAT_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-6".to_string()),
        max_tokens: std::env::var("SCITADEL_CHAT_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4096),
        scoring_concurrency: std::env::var("SCITADEL_SCORING_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5),
    };

    // Try loading TOML config file
    let config_path = workspace.join(".scitadel").join("config.toml");

    if let Some(mut config) = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|contents| toml::from_str::<Config>(&contents).ok())
    {
        // Env vars override TOML
        config.db_path = db_path;
        if !pubmed.api_key.is_empty() {
            config.pubmed.api_key = pubmed.api_key;
        }
        if !openalex.api_key.is_empty() {
            config.openalex.api_key = openalex.api_key;
        }
        config.chat = chat;
        return config;
    }

    Config {
        db_path,
        pubmed,
        openalex,
        chat,
        ..Default::default()
    }
}

/// Load config from a specific TOML file path.
pub fn load_config_from(path: &Path) -> Result<Config, crate::error::CoreError> {
    let contents = std::fs::read_to_string(path)?;
    toml::from_str(&contents).map_err(|e| crate::error::CoreError::Config(e.to_string()))
}
