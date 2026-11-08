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
    /// Alternative endpoint for a source whose API has more than one host
    /// speaking the same grammar. Only `patentsview` honours it today
    /// (#268); `None` keeps the adapter's built-in default.
    #[serde(default)]
    pub base_url: Option<String>,
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: 30.0,
            max_retries: 3,
            api_key: String::new(),
            base_url: None,
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

/// EPO OPS adapter configuration (requires consumer key + secret pair).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpoConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timeout")]
    pub timeout: f64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub consumer_key: String,
    #[serde(default)]
    pub consumer_secret: String,
}

impl Default for EpoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: 30.0,
            max_retries: 3,
            consumer_key: String::new(),
            consumer_secret: String::new(),
        }
    }
}

/// Resolved OpenAlex credentials, carried as one value so the adapter,
/// the download chain and the TUI can't drift apart on which half they
/// were handed (#212).
///
/// * `email` — polite-pool `mailto` contact address (also used for Unpaywall).
/// * `api_key` — the metered key OpenAlex requires once the shared
///   per-IP budget is spent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenAlexAuth {
    pub email: String,
    pub api_key: String,
}

impl OpenAlexAuth {
    /// True when neither credential is set — the fully anonymous path
    /// that OpenAlex now meters against a shared per-IP daily budget.
    pub fn is_anonymous(&self) -> bool {
        self.email.is_empty() && self.api_key.is_empty()
    }
}

/// OpenAlex adapter configuration.
///
/// OpenAlex needs two *different* credentials, which scitadel <= 0.7
/// conflated into a single `api_key` field that actually held the
/// polite-pool email. They are separate keys now: `email` is the
/// `mailto` contact address, `api_key` is the real metered API key.
/// Configs written by older versions still load — see
/// [`OpenAlexConfig::migrate_legacy_email`] (#212).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAlexConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timeout")]
    pub timeout: f64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Polite-pool contact address, sent as `mailto=`.
    #[serde(default)]
    pub email: String,
    /// Metered API key, sent as `api_key=`.
    #[serde(default)]
    pub api_key: String,
}

impl Default for OpenAlexConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: 30.0,
            max_retries: 3,
            email: String::new(),
            api_key: String::new(),
        }
    }
}

impl OpenAlexConfig {
    /// Move a legacy `api_key = "me@example.org"` into `email`.
    ///
    /// Pre-0.8 `config.toml` files stored the polite-pool email under
    /// `api_key`. An `@` is the discriminant: OpenAlex API keys are
    /// opaque tokens and never contain one, while every email address
    /// does. Only applied when `email` is still unset, so a config that
    /// sets both keys explicitly is left alone.
    pub fn migrate_legacy_email(&mut self) {
        if self.email.is_empty() && self.api_key.contains('@') {
            self.email = std::mem::take(&mut self.api_key);
        }
    }

    /// The credential pair this config resolves to.
    pub fn auth(&self) -> OpenAlexAuth {
        OpenAlexAuth {
            email: self.email.clone(),
            api_key: self.api_key.clone(),
        }
    }
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

/// UI/UX preferences (TUI-only today; extensible for future surfaces).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// When a download lands on a paywalled publisher page, show the live URL
    /// plus a note that an institutional IP range (e.g. university VPN) may
    /// grant access. Disable for headless / CI runs.
    #[serde(default = "default_true")]
    pub show_institutional_hint: bool,
    /// Active TUI theme (#137). Accepts: `auto`, `dark`, `light`,
    /// `dalton-dark`, `dalton-bright`. `auto` = read terminal
    /// background via COLORFGBG → OSC 11 → fall back to dark.
    /// Resolution order: CLI flag > `SCITADEL_THEME` env > this
    /// config key > `auto`.
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "auto".into()
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_institutional_hint: true,
            theme: default_theme(),
        }
    }
}

/// Where the resolved `patentsview.api_key` came from (#268).
///
/// Drives the source-aware remediation hint in
/// [`scitadel_adapters::enforce_patentsview_host_binding`]: a stored
/// key is rebound with `scitadel auth login patentsview --host <h>`,
/// while an env-set key is only ever fixed by changing env (set
/// `SCITADEL_PATENTSVIEW_URL`) or by clearing the file-set `base_url`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PatentsViewKeySource {
    /// No key configured.
    #[default]
    None,
    /// Secret store (keychain / Secret Service / 0600 file backend).
    Stored,
    /// `SCITADEL_PATENTSVIEW_KEY` in the process environment.
    Env,
    /// `[patentsview] api_key` in `.scitadel/config.toml`.
    File,
}

/// Fully-resolved PatentsView adapter configuration (#268).
///
/// Carries the `api_key` plus the host it is allowed to reach, so a
/// malicious `[patentsview] base_url` in a repo-level `.scitadel/config.toml`
/// cannot replay a keychain-stored key to an attacker-controlled host.
///
/// The field `bound_host` works as follows:
///
/// * `Some(h)` — the key is bound to that host and the adapter builder
///   refuses to construct an adapter whose effective base-URL host is
///   anything else. A legacy keychain entry with no stored host is
///   treated as bound to the retired USPTO PatentSearch host.
/// * `None` — no binding check. Reserved for env-only keys with an
///   env-or-default URL (an explicit per-process choice the operator
///   made), and for config-file keys whose base_url sits in the same
///   file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatentsViewAdapterConfig {
    /// Resolved key (keychain → env → config-file fallback → empty).
    pub api_key: String,
    /// Effective base URL (env override → config file → `None` = adapter default).
    pub base_url: Option<String>,
    /// Host the key is bound to. See the struct docs.
    pub bound_host: Option<String>,
    /// `bound_host` was derived from a legacy keychain entry with no
    /// stored host, so the error should point at the PatentRef migration.
    pub legacy_unbound: bool,
    /// Where `api_key` came from — drives the source-aware remediation
    /// hint when the binding refuses.
    pub key_source: PatentsViewKeySource,
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
    pub openalex: OpenAlexConfig,
    #[serde(default)]
    pub inspire: SourceConfig,
    #[serde(default)]
    pub patentsview: SourceConfig,
    #[serde(default)]
    pub lens: SourceConfig,
    #[serde(default)]
    pub epo: EpoConfig,
    #[serde(default)]
    pub chat: ChatConfig,
    #[serde(default)]
    pub ui: UiConfig,
    /// Fully-resolved PatentsView adapter config (#268). Not serialized;
    /// filled by [`load_config`] from the keychain-stored host binding,
    /// env overrides and the raw config file.
    #[serde(skip)]
    pub patentsview_adapter: PatentsViewAdapterConfig,
}

fn default_sources() -> Vec<String> {
    vec![
        "pubmed".into(),
        "arxiv".into(),
        "openalex".into(),
        "inspire".into(),
    ]
}

impl Config {
    /// Directory for downloaded paper files, relative to the database location.
    pub fn papers_dir(&self) -> PathBuf {
        self.db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("papers")
    }
}

impl Default for Config {
    fn default() -> Self {
        let workspace = find_workspace_root().unwrap_or_else(|| std::env::current_dir().unwrap());
        Self {
            db_path: default_db_path(&workspace),
            default_sources: default_sources(),
            pubmed: SourceConfig::default(),
            arxiv: SourceConfig::default(),
            openalex: OpenAlexConfig::default(),
            inspire: SourceConfig::default(),
            patentsview: SourceConfig::default(),
            lens: SourceConfig::default(),
            epo: EpoConfig::default(),
            chat: ChatConfig::default(),
            ui: UiConfig::default(),
            patentsview_adapter: PatentsViewAdapterConfig::default(),
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

/// The per-process inputs [`resolve_patentsview_adapter`] uses besides
/// the TOML file (#268).
///
/// Factored out so the resolver is driven by a value, not by hidden
/// reads of `std::env` and the secret store — the function is then
/// table-testable end-to-end without poking the process environment.
///
/// `from_process()` reads the real env and the active secret store;
/// tests build this struct inline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatentsViewEnv {
    /// Value in the active secret store under `patentsview.api_key`,
    /// or `None`.
    pub stored_key: Option<String>,
    /// Value in the active secret store under
    /// [`crate::credentials::PATENTSVIEW_HOST_STORE_KEY`], or `None`.
    /// Absence with `stored_key = Some(..)` is the "legacy unbound"
    /// case: the key was stored before host binding existed and is
    /// treated as pointing at the retired USPTO host.
    pub stored_host: Option<String>,
    /// `SCITADEL_PATENTSVIEW_KEY` in the process environment, trimmed,
    /// `None` if empty or unset.
    pub env_key: Option<String>,
    /// `SCITADEL_PATENTSVIEW_URL` in the process environment, trimmed,
    /// `None` if empty or unset.
    pub env_url: Option<String>,
}

impl PatentsViewEnv {
    /// Read the real process environment and the active secret store.
    ///
    /// The public [`resolve_patentsview_adapter`] uses this; tests
    /// construct the struct themselves instead.
    pub fn from_process() -> Self {
        let stored_key = crate::credentials::get("patentsview.api_key").map(|s| s.into_string());
        let stored_host = crate::credentials::get(crate::credentials::PATENTSVIEW_HOST_STORE_KEY)
            .map(|s| s.into_string().trim().to_string())
            .filter(|s| !s.is_empty());
        let env_key = std::env::var("SCITADEL_PATENTSVIEW_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let env_url = std::env::var("SCITADEL_PATENTSVIEW_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Self {
            stored_key,
            stored_host,
            env_key,
            env_url,
        }
    }
}

/// Resolve the PatentsView adapter configuration (#268).
///
/// Reads the process environment and the active secret store via
/// [`PatentsViewEnv::from_process`]. Thin wrapper over
/// [`resolve_patentsview_adapter_with_env`], which is what the tests
/// drive directly.
pub fn resolve_patentsview_adapter(
    file_api_key: &str,
    file_base_url: Option<&str>,
) -> PatentsViewAdapterConfig {
    resolve_patentsview_adapter_with_env(
        &PatentsViewEnv::from_process(),
        file_api_key,
        file_base_url,
    )
}

/// Resolve the PatentsView adapter configuration from explicit inputs
/// (#268).
///
/// The patentsview key is a bearer token sent as `X-Api-Key`, and
/// `[patentsview] base_url` can be committed in a repo-level
/// `.scitadel/config.toml`. The combination is dangerous: a cloned
/// repo could point the adapter at an attacker-controlled host and
/// collect any key the current user has stored. The mitigation is to
/// **bind** the key to a host at store time (`scitadel auth login
/// patentsview [--host <host>]`) and refuse to send it to any other
/// host at adapter-build time.
///
/// # Parameters
///
/// * `env` — process env + secret store state, see [`PatentsViewEnv`].
/// * `file_api_key` — `[patentsview] api_key` as it appeared in the
///   TOML file (or `""` if absent). Kept as a fallback so a user who
///   writes the key in their own config.toml is not pushed into the
///   keychain.
/// * `file_base_url` — `[patentsview] base_url` as it appeared in the
///   TOML file, BEFORE applying the `SCITADEL_PATENTSVIEW_URL` env
///   override. Knowing it came from the file is load-bearing: a
///   base-URL in the file is the one case where an env-set key must
///   refuse to send (the file could have been committed by an attacker).
///
/// # Returned binding
///
/// | key source | URL source | `bound_host` | `legacy_unbound` | `key_source` |
/// |------------|------------|--------------|------------------|--------------|
/// | stored + stored host | — | `Some(stored_host)` | `false` | `Stored` |
/// | stored, no stored host | — | `Some(LEGACY_HOST)` | `true` | `Stored` |
/// | env | env or default | `None` | `false` | `Env` |
/// | env | file | `Some(DEFAULT_HOST)` [^1] | `false` | `Env` |
/// | file | — | `None` | `false` | `File` |
/// | none | — | `None` | `false` | `None` |
///
/// [^1]: forces the adapter builder to refuse against any file-set
///       URL that is not the default host; the operator has to either
///       clear the file's `base_url` or bind the key explicitly.
pub fn resolve_patentsview_adapter_with_env(
    env: &PatentsViewEnv,
    file_api_key: &str,
    file_base_url: Option<&str>,
) -> PatentsViewAdapterConfig {
    // 1. Effective base URL: env wins over file, which wins over
    //    "none" (the adapter then uses its built-in default).
    let env_url = env.env_url.clone();
    let file_base_url = file_base_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let effective_base_url = env_url.clone().or_else(|| file_base_url.clone());

    let file_key = Some(file_api_key.trim().to_string()).filter(|s| !s.is_empty());

    if let Some(key) = env.stored_key.clone() {
        // Stored key — the binding (if any) travels with it. A stored
        // key with no stored host is a legacy entry written before
        // host-binding existed; treat it as USPTO so the error
        // message can be migration-specific.
        let legacy_unbound = env.stored_host.is_none();
        let bound_host = env
            .stored_host
            .clone()
            .unwrap_or_else(|| crate::credentials::PATENTSVIEW_LEGACY_HOST.to_string());
        return PatentsViewAdapterConfig {
            api_key: key,
            base_url: effective_base_url,
            bound_host: Some(bound_host),
            legacy_unbound,
            key_source: PatentsViewKeySource::Stored,
        };
    }

    if let Some(key) = env.env_key.clone() {
        // Env key: an explicit per-process choice the operator made.
        // Honour it when the URL is also from env or the default,
        // but refuse to let it ride a `base_url` set in a repo file —
        // that file could be committed by someone else.
        let bound_host = if env_url.is_some() || file_base_url.is_none() {
            None
        } else {
            // base_url comes from the file and no env override is set
            // — the adapter builder must refuse against any non-default
            // host.
            Some(crate::credentials::PATENTSVIEW_DEFAULT_HOST.to_string())
        };
        return PatentsViewAdapterConfig {
            api_key: key,
            base_url: effective_base_url,
            bound_host,
            legacy_unbound: false,
            key_source: PatentsViewKeySource::Env,
        };
    }

    if let Some(key) = file_key {
        // Key in the same config.toml the user wrote themselves. If
        // they set a different host there, that is their choice; no
        // binding check is enforced. The https-only validator in the
        // adapter builder still rejects plaintext non-loopback URLs.
        return PatentsViewAdapterConfig {
            api_key: key,
            base_url: effective_base_url,
            bound_host: None,
            legacy_unbound: false,
            key_source: PatentsViewKeySource::File,
        };
    }

    PatentsViewAdapterConfig {
        api_key: String::new(),
        base_url: effective_base_url,
        bound_host: None,
        legacy_unbound: false,
        key_source: PatentsViewKeySource::None,
    }
}

/// Load configuration from keychain, environment variables, and optional TOML file.
///
/// Resolution priority per credential: keychain → env var → config.toml → empty default.
pub fn load_config() -> Config {
    let workspace = find_workspace_root().unwrap_or_else(|| std::env::current_dir().unwrap());
    load_config_with(&workspace, &PatentsViewEnv::from_process())
}

/// Load configuration with an explicit workspace root and patentsview
/// env snapshot (#268).
///
/// The split exists so the round-2 attack path — stored key +
/// attacker-committed `[patentsview] base_url` in a repo-level
/// `.scitadel/config.toml` — can be driven end-to-end from a test with
/// a temp workspace and a synthetic [`PatentsViewEnv`], without
/// mutating the real process environment or the real secret store.
/// Every other credential still reads from the real environment; the
/// test can leave those unset.
pub fn load_config_with(workspace: &Path, pv_env: &PatentsViewEnv) -> Config {
    use crate::credentials::resolve;

    let db_path = default_db_path(workspace);

    // Try loading TOML config file as base
    let config_path = workspace.join(".scitadel").join("config.toml");
    let mut config: Config = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default();

    config.db_path = db_path;

    // Resolve credentials: keychain → env → config.toml value
    config.pubmed.api_key = resolve(
        "pubmed.api_key",
        "SCITADEL_PUBMED_API_KEY",
        &config.pubmed.api_key,
    )
    .map(crate::credentials::Secret::into_string)
    .unwrap_or_default();

    // Untangle the pre-0.8 layout (email stored under `api_key`) before
    // either field is used as a fallback for its own credential (#212).
    config.openalex.migrate_legacy_email();

    config.openalex.email = resolve(
        "openalex.email",
        "SCITADEL_OPENALEX_EMAIL",
        &config.openalex.email,
    )
    .map(crate::credentials::Secret::into_string)
    .unwrap_or_default();

    config.openalex.api_key = resolve(
        "openalex.api_key",
        "SCITADEL_OPENALEX_API_KEY",
        &config.openalex.api_key,
    )
    .map(crate::credentials::Secret::into_string)
    .unwrap_or_default();

    // The patentsview key is bound to a host at store time (#268), so
    // its resolution is more involved than the other sources. The
    // helper picks a source for the key, decides which host it is
    // allowed to reach, and tracks whether the stored key is a
    // legacy (pre-binding) entry so the error points at the migration.
    //
    // `config.patentsview.api_key` and `config.patentsview.base_url`
    // keep the resolved values for callers that read them directly
    // (e.g. the `!is_empty()` check in the MCP source listing); the
    // full resolution, including `bound_host`, lives on
    // `config.patentsview_adapter`.
    let file_base_url = config.patentsview.base_url.clone();
    let file_api_key = config.patentsview.api_key.clone();
    let adapter_cfg =
        resolve_patentsview_adapter_with_env(pv_env, &file_api_key, file_base_url.as_deref());
    config.patentsview.api_key.clone_from(&adapter_cfg.api_key);
    config
        .patentsview
        .base_url
        .clone_from(&adapter_cfg.base_url);
    config.patentsview_adapter = adapter_cfg;

    config.lens.api_key = resolve(
        "lens.api_token",
        "SCITADEL_LENS_TOKEN",
        &config.lens.api_key,
    )
    .map(crate::credentials::Secret::into_string)
    .unwrap_or_default();

    config.epo.consumer_key = resolve(
        "epo.consumer_key",
        "SCITADEL_EPO_KEY",
        &config.epo.consumer_key,
    )
    .map(crate::credentials::Secret::into_string)
    .unwrap_or_default();

    config.epo.consumer_secret = resolve(
        "epo.consumer_secret",
        "SCITADEL_EPO_SECRET",
        &config.epo.consumer_secret,
    )
    .map(crate::credentials::Secret::into_string)
    .unwrap_or_default();

    // Chat config from env (no keychain needed)
    if let Ok(model) = std::env::var("SCITADEL_CHAT_MODEL") {
        config.chat.model = model;
    }
    if let Ok(tokens) = std::env::var("SCITADEL_CHAT_MAX_TOKENS")
        && let Ok(v) = tokens.parse()
    {
        config.chat.max_tokens = v;
    }
    if let Ok(conc) = std::env::var("SCITADEL_SCORING_CONCURRENCY")
        && let Ok(v) = conc.parse()
    {
        config.chat.scoring_concurrency = v;
    }

    config
}

/// Load config from a specific TOML file path.
pub fn load_config_from(path: &Path) -> Result<Config, crate::error::CoreError> {
    let contents = std::fs::read_to_string(path)?;
    let mut config: Config =
        toml::from_str(&contents).map_err(|e| crate::error::CoreError::Config(e.to_string()))?;
    config.openalex.migrate_legacy_email();
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openalex_from_toml(body: &str) -> OpenAlexConfig {
        let mut cfg: OpenAlexConfig = toml::from_str(body).unwrap();
        cfg.migrate_legacy_email();
        cfg
    }

    #[test]
    fn a_pre_0_8_config_keeps_working() {
        // scitadel <= 0.7 wrote the polite-pool email into `api_key` (#212).
        let cfg = openalex_from_toml(r#"api_key = "lars@example.org""#);
        assert_eq!(cfg.email, "lars@example.org");
        assert_eq!(cfg.api_key, "", "an email is not an API key");
    }

    #[test]
    fn a_real_api_key_is_left_where_it_is() {
        let cfg = openalex_from_toml(r#"api_key = "oa-key-123""#);
        assert_eq!(cfg.api_key, "oa-key-123");
        assert_eq!(cfg.email, "");
    }

    #[test]
    fn an_explicit_pair_is_never_rewritten() {
        let cfg = openalex_from_toml(
            r#"
            email = "lars@example.org"
            api_key = "oa-key-123"
            "#,
        );
        assert_eq!(cfg.email, "lars@example.org");
        assert_eq!(cfg.api_key, "oa-key-123");
    }

    #[test]
    fn migration_does_not_clobber_an_email_that_is_already_set() {
        // Pathological but possible: both fields hold addresses.
        let cfg = openalex_from_toml(
            r#"
            email = "new@example.org"
            api_key = "old@example.org"
            "#,
        );
        assert_eq!(cfg.email, "new@example.org");
        assert_eq!(cfg.api_key, "old@example.org");
    }

    #[test]
    fn migration_is_idempotent() {
        let mut cfg = openalex_from_toml(r#"api_key = "lars@example.org""#);
        cfg.migrate_legacy_email();
        cfg.migrate_legacy_email();
        assert_eq!(cfg.email, "lars@example.org");
        assert_eq!(cfg.api_key, "");
    }

    #[test]
    fn auth_carries_both_halves() {
        let cfg = openalex_from_toml(
            r#"
            email = "lars@example.org"
            api_key = "oa-key-123"
            "#,
        );
        let auth = cfg.auth();
        assert_eq!(auth.email, "lars@example.org");
        assert_eq!(auth.api_key, "oa-key-123");
        assert!(!auth.is_anonymous());
        assert!(OpenAlexAuth::default().is_anonymous());
    }

    #[test]
    fn load_config_from_migrates_a_legacy_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "db_path = \"/tmp/x.db\"\n\n[openalex]\napi_key = \"lars@example.org\"\n",
        )
        .unwrap();

        let config = load_config_from(&path).unwrap();
        assert_eq!(config.openalex.email, "lars@example.org");
        assert_eq!(config.openalex.api_key, "");
    }

    #[test]
    fn a_config_without_an_openalex_section_defaults_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "db_path = \"/tmp/x.db\"\n").unwrap();

        let config = load_config_from(&path).unwrap();
        assert!(config.openalex.auth().is_anonymous());
        assert!(config.openalex.enabled);
        assert!((config.openalex.timeout - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_source_base_url_is_optional_and_read_from_toml() {
        let unset: SourceConfig = toml::from_str(r#"api_key = "k""#).unwrap();
        assert_eq!(unset.base_url, None);

        let set: SourceConfig =
            toml::from_str(r#"base_url = "https://mirror.example.org/api/v1/patent/""#).unwrap();
        assert_eq!(
            set.base_url.as_deref(),
            Some("https://mirror.example.org/api/v1/patent/")
        );
    }

    // ---------------------------------------------------------------
    // resolve_patentsview_adapter_with_env (#268)
    //
    // Every row of the resolver's dispatch table has a test, so that
    // adding a row later (e.g. an MCP-tool-provided key) fails loudly
    // if it misses its binding or its key_source. The CLI's host-bind
    // round-trip through the secret store is already covered by
    // `credentials::tests::the_patentsview_host_binding_round_trips_through_the_file_backend`.
    // ---------------------------------------------------------------

    fn env(
        stored_key: Option<&str>,
        stored_host: Option<&str>,
        env_key: Option<&str>,
        env_url: Option<&str>,
    ) -> PatentsViewEnv {
        PatentsViewEnv {
            stored_key: stored_key.map(str::to_string),
            stored_host: stored_host.map(str::to_string),
            env_key: env_key.map(str::to_string),
            env_url: env_url.map(str::to_string),
        }
    }

    #[test]
    fn stored_key_with_default_url_binds_to_stored_host() {
        let e = env(Some("stored-k"), Some("patentref.io"), None, None);
        let got = resolve_patentsview_adapter_with_env(&e, "", None);
        assert_eq!(got.api_key, "stored-k");
        assert_eq!(got.base_url, None);
        assert_eq!(got.bound_host.as_deref(), Some("patentref.io"));
        assert!(!got.legacy_unbound);
        assert_eq!(got.key_source, PatentsViewKeySource::Stored);
    }

    #[test]
    fn stored_key_with_file_url_matching_bound_host_builds() {
        // Store-time binding was `patentref.io`; the file URL names the
        // same host. The resolver lets both through — the adapter
        // builder's host comparison is what decides.
        let e = env(Some("stored-k"), Some("patentref.io"), None, None);
        let got = resolve_patentsview_adapter_with_env(
            &e,
            "",
            Some("https://patentref.io/api/v1/patent/"),
        );
        assert_eq!(got.api_key, "stored-k");
        assert_eq!(
            got.base_url.as_deref(),
            Some("https://patentref.io/api/v1/patent/")
        );
        assert_eq!(got.bound_host.as_deref(), Some("patentref.io"));
        assert_eq!(got.key_source, PatentsViewKeySource::Stored);
    }

    #[test]
    fn stored_key_with_file_url_mismatching_bound_host_still_carries_the_binding() {
        // The resolver carries the stored binding through — the
        // adapter builder refuses at search time. We assert the shape
        // the builder sees.
        let e = env(Some("stored-k"), Some("patentref.io"), None, None);
        let got = resolve_patentsview_adapter_with_env(
            &e,
            "",
            Some("https://attacker.example/api/v1/patent/"),
        );
        assert_eq!(got.api_key, "stored-k");
        assert_eq!(
            got.base_url.as_deref(),
            Some("https://attacker.example/api/v1/patent/")
        );
        assert_eq!(got.bound_host.as_deref(), Some("patentref.io"));
    }

    #[test]
    fn stored_key_with_env_url_still_binds_to_stored_host() {
        // An env URL does not loosen the stored binding — the key was
        // bound at store time for a reason.
        let e = env(
            Some("stored-k"),
            Some("patentref.io"),
            None,
            Some("https://mirror.example/api/v1/patent/"),
        );
        let got = resolve_patentsview_adapter_with_env(&e, "", None);
        assert_eq!(
            got.base_url.as_deref(),
            Some("https://mirror.example/api/v1/patent/")
        );
        assert_eq!(got.bound_host.as_deref(), Some("patentref.io"));
    }

    #[test]
    fn env_key_with_env_url_runs_unbound() {
        // Operator chose both the key and the URL explicitly this
        // invocation — no file was involved.
        let e = env(
            None,
            None,
            Some("env-k"),
            Some("https://mirror.example/api/v1/patent/"),
        );
        let got = resolve_patentsview_adapter_with_env(&e, "", None);
        assert_eq!(got.api_key, "env-k");
        assert_eq!(
            got.base_url.as_deref(),
            Some("https://mirror.example/api/v1/patent/")
        );
        assert_eq!(got.bound_host, None);
        assert_eq!(got.key_source, PatentsViewKeySource::Env);
    }

    #[test]
    fn env_key_with_default_url_runs_unbound() {
        let e = env(None, None, Some("env-k"), None);
        let got = resolve_patentsview_adapter_with_env(&e, "", None);
        assert_eq!(got.api_key, "env-k");
        assert_eq!(got.base_url, None);
        assert_eq!(got.bound_host, None);
        assert_eq!(got.key_source, PatentsViewKeySource::Env);
    }

    #[test]
    fn env_key_with_file_url_refuses_via_default_host_binding() {
        // THE round-2 attack mitigation on the env-key path: a
        // committed `[patentsview] base_url` is not authority for an
        // env-set key to reach that host. The resolver binds the key
        // to the DEFAULT host, so the adapter builder refuses any
        // non-default `base_url`.
        let e = env(None, None, Some("env-k"), None);
        let got = resolve_patentsview_adapter_with_env(
            &e,
            "",
            Some("https://attacker.example/api/v1/patent/"),
        );
        assert_eq!(got.api_key, "env-k");
        assert_eq!(
            got.base_url.as_deref(),
            Some("https://attacker.example/api/v1/patent/")
        );
        assert_eq!(
            got.bound_host.as_deref(),
            Some(crate::credentials::PATENTSVIEW_DEFAULT_HOST),
        );
        assert_eq!(got.key_source, PatentsViewKeySource::Env);
    }

    #[test]
    fn legacy_stored_key_without_binding_is_treated_as_uspto() {
        // A `patentsview.api_key` written before #268 existed has no
        // companion host entry. Treat it as bound to the retired USPTO
        // host so the adapter builder's error points at the migration.
        let e = env(Some("legacy-uspto-k"), None, None, None);
        let got = resolve_patentsview_adapter_with_env(&e, "", None);
        assert_eq!(got.api_key, "legacy-uspto-k");
        assert_eq!(
            got.bound_host.as_deref(),
            Some(crate::credentials::PATENTSVIEW_LEGACY_HOST),
        );
        assert!(got.legacy_unbound);
        assert_eq!(got.key_source, PatentsViewKeySource::Stored);
    }

    #[test]
    fn file_key_runs_unbound() {
        // The user wrote both the key and the URL into their own
        // config.toml — the decision is theirs.
        let e = env(None, None, None, None);
        let got = resolve_patentsview_adapter_with_env(
            &e,
            "file-k",
            Some("https://mirror.example/api/v1/patent/"),
        );
        assert_eq!(got.api_key, "file-k");
        assert_eq!(got.bound_host, None);
        assert_eq!(got.key_source, PatentsViewKeySource::File);
    }

    #[test]
    fn no_key_anywhere_yields_a_disabled_config() {
        let e = env(None, None, None, None);
        let got = resolve_patentsview_adapter_with_env(&e, "", None);
        assert_eq!(got.api_key, "");
        assert_eq!(got.bound_host, None);
        assert!(!got.legacy_unbound);
        assert_eq!(got.key_source, PatentsViewKeySource::None);
    }

    #[test]
    fn the_attacker_committed_base_url_against_a_stored_key_refuses_adapter_build() {
        // End-to-end round-2 attack path: a cloned repo commits
        // `.scitadel/config.toml` with `[patentsview] base_url` pointing
        // at an attacker-controlled host, AND the operator already has
        // a `patentsview.api_key` bound to `patentref.io`. `load_config`
        // must surface a `bound_host = Some("patentref.io")`, and
        // feeding that into `build_adapters_with_field` must refuse
        // without ever building an adapter (zero requests). The
        // resolver's binding is what guarantees that.
        let dir = tempfile::tempdir().unwrap();
        let scitadel_dir = dir.path().join(".scitadel");
        std::fs::create_dir_all(&scitadel_dir).unwrap();
        std::fs::write(
            scitadel_dir.join("config.toml"),
            r#"db_path = "scitadel.db"

[patentsview]
base_url = "https://attacker.example/api/v1/patent/"
"#,
        )
        .unwrap();

        let pv_env = env(Some("stored-k"), Some("patentref.io"), None, None);
        let config = load_config_with(dir.path(), &pv_env);

        assert_eq!(config.patentsview_adapter.api_key, "stored-k");
        assert_eq!(
            config.patentsview_adapter.base_url.as_deref(),
            Some("https://attacker.example/api/v1/patent/"),
        );
        assert_eq!(
            config.patentsview_adapter.bound_host.as_deref(),
            Some("patentref.io"),
        );
        assert!(!config.patentsview_adapter.legacy_unbound);
        assert_eq!(
            config.patentsview_adapter.key_source,
            PatentsViewKeySource::Stored,
        );
        // The round-trip through `build_adapters_with_field` lives in
        // the scitadel-adapters test suite
        // (`patentsview_http::a_bound_key_refuses_a_different_configured_host_and_makes_no_request`);
        // the shape we assert here is the input that drives that
        // refusal.
    }
}
