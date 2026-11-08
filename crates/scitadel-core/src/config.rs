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

/// Resolve the PatentsView adapter configuration (#268).
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
/// | key source | URL source | `bound_host` | `legacy_unbound` |
/// |------------|------------|--------------|------------------|
/// | keychain + stored host | — | `Some(stored_host)` | `false` |
/// | keychain, no stored host | — | `Some(LEGACY_HOST)` | `true` |
/// | env | env or default | `None` | `false` |
/// | env | file | `Some(DEFAULT_HOST)` [^1] | `false` |
/// | file | — | `None` | `false` |
/// | none | — | `None` | `false` |
///
/// [^1]: forces the adapter builder to refuse against any file-set
///       URL that is not the default host; the operator has to either
///       clear the file's `base_url` or bind the key explicitly.
pub fn resolve_patentsview_adapter(
    file_api_key: &str,
    file_base_url: Option<&str>,
) -> PatentsViewAdapterConfig {
    // 1. Effective base URL: env wins over file, which wins over
    //    "none" (the adapter then uses its built-in default).
    let env_url = std::env::var("SCITADEL_PATENTSVIEW_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let file_base_url = file_base_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let effective_base_url = env_url.clone().or_else(|| file_base_url.clone());

    // 2. Pick the key source. `credentials::get` only inspects the
    //    active secret store, so we can distinguish keychain from env
    //    from file-fallback instead of collapsing them with `resolve`.
    let stored_key = crate::credentials::get("patentsview.api_key").map(|s| s.into_string());
    let env_key = std::env::var("SCITADEL_PATENTSVIEW_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let file_key = Some(file_api_key.trim().to_string()).filter(|s| !s.is_empty());

    if let Some(key) = stored_key {
        // Stored key — the binding (if any) travels with it. A stored
        // key with no stored host is a legacy entry written before
        // host-binding existed; treat it as USPTO so the error
        // message can be migration-specific.
        let stored_host = crate::credentials::get(crate::credentials::PATENTSVIEW_HOST_STORE_KEY)
            .map(|s| s.into_string().trim().to_string())
            .filter(|s| !s.is_empty());
        let legacy_unbound = stored_host.is_none();
        let bound_host =
            stored_host.unwrap_or_else(|| crate::credentials::PATENTSVIEW_LEGACY_HOST.to_string());
        return PatentsViewAdapterConfig {
            api_key: key,
            base_url: effective_base_url,
            bound_host: Some(bound_host),
            legacy_unbound,
        };
    }

    if let Some(key) = env_key {
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
        };
    }

    PatentsViewAdapterConfig {
        api_key: String::new(),
        base_url: effective_base_url,
        bound_host: None,
        legacy_unbound: false,
    }
}

/// Load configuration from keychain, environment variables, and optional TOML file.
///
/// Resolution priority per credential: keychain → env var → config.toml → empty default.
pub fn load_config() -> Config {
    use crate::credentials::resolve;

    let workspace = find_workspace_root().unwrap_or_else(|| std::env::current_dir().unwrap());
    let db_path = default_db_path(&workspace);

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
    let adapter_cfg = resolve_patentsview_adapter(&file_api_key, file_base_url.as_deref());
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
}
