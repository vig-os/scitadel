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

/// Per-run tuning for `acquire` and the DOI-only `download` path — the two
/// knobs a library operator can turn without a rebuild.
///
/// Today one knob: the fetch-body cap (#288). Future knobs for the acquire
/// slice land here rather than growing their own top-level sections, so the
/// TOML `[acquire]` table stays the one place to look.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcquireConfig {
    /// Override for the per-fetch body cap (ADR-007 §1 "Storage"; #288),
    /// in **bytes**.
    ///
    /// `None` — the serde default — keeps the workspace-wide constant
    /// `scitadel_adapters::download::FETCH_BODY_CAP_BYTES` (which is the
    /// one re-export of `cap_for_kind("fulltext_pdf")`). Setting a value
    /// **raises or lowers** the fulltext row of ADR-007 §1's cap table in
    /// one place: `scitadel_db::sqlite::cap_for_kind_with_override` reads
    /// it, and *both* the fetch path (`scitadel_adapters::download`) and
    /// the drop-in path (`scitadel_adapters::scan::{scan, plan, attach}`)
    /// pass the same `body_cap_bytes` through it, so a manual import of an
    /// oversized fulltext is admitted iff a fetch of the same bytes would
    /// be. SI / figure / table caps are not touched: the knob names *the
    /// fetch body cap* and the fetch body is a fulltext.
    ///
    /// Raising the cap is how an operator unlocks a cap-refused work for a
    /// named re-acquire (`scitadel acquire --paper <id>`), which is the one
    /// way out of `too_large` (ADR-007 §2 doc / migration 015); "fetch
    /// manually and import" works for the same reason, now that scan reads
    /// the same cap.
    ///
    /// Env override: `SCITADEL_ACQUIRE_BODY_CAP_BYTES` (parsed as `u64`).
    /// Precedence: env > TOML > unset (keep the default). The value is
    /// validated at load time against [`BODY_CAP_MIN_BYTES`] — a cap under
    /// 1 MiB would refuse every full-text PDF the ladder ever fetches, so
    /// values under the floor (and the "`100` meant as MB" mistake) are
    /// rejected rather than silently applied.
    #[serde(default)]
    pub body_cap_bytes: Option<u64>,
}

/// The lowest `body_cap_bytes` the loader accepts, in bytes (#288).
///
/// 1 MiB — a cap below this would refuse every real full-text PDF. The floor
/// exists to catch the "`100` meant as MB" mistake at load time rather than
/// discovering it when every fetch lands in terminal `too_large`.
pub const BODY_CAP_MIN_BYTES: u64 = 1024 * 1024;

impl AcquireConfig {
    /// Validate the acquire knobs against their valid ranges (#288).
    ///
    /// Today one check — `body_cap_bytes >= BODY_CAP_MIN_BYTES` when set. The
    /// error names the key, its unit (bytes), the valid range, and the common
    /// "I meant megabytes" mistake, because the error message is the one the
    /// operator sees when the CLI refuses to start.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(bytes) = self.body_cap_bytes
            && bytes < BODY_CAP_MIN_BYTES
        {
            return Err(format!(
                "acquire.body_cap_bytes = {bytes} is below the {min} B floor; \
                 this knob is measured in **bytes** (not MB), so a cap under \
                 1 MiB would refuse every full-text PDF the ladder fetches. \
                 Valid range: {min}..=u64::MAX (bytes). For 100 MB write \
                 acquire.body_cap_bytes = {hundred_mb}.",
                min = BODY_CAP_MIN_BYTES,
                hundred_mb = 100 * 1024 * 1024_u64,
            ));
        }
        Ok(())
    }
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
    #[serde(default)]
    pub acquire: AcquireConfig,
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
            acquire: AcquireConfig::default(),
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

    // Enforce the acquire-knob floor from the TOML side (#288). The infallible
    // loader can't return `Err`, but a sub-floor value would otherwise land
    // every fetch in terminal `too_large`; warn and discard the override so the
    // CLI starts on the safe default instead.
    if let Err(msg) = config.acquire.validate() {
        tracing::warn!(error = %msg, "acquire config invalid; keeping the default");
        config.acquire.body_cap_bytes = None;
    }

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

    config.patentsview.api_key = resolve(
        "patentsview.api_key",
        "SCITADEL_PATENTSVIEW_KEY",
        &config.patentsview.api_key,
    )
    .map(crate::credentials::Secret::into_string)
    .unwrap_or_default();

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

    // Acquire knobs (#288). `None` in the TOML keeps the workspace-wide
    // default; a parse failure *or* a sub-floor value is ignored rather than
    // surfaced so a typo never stops the CLI starting — the warning goes
    // through `tracing` and the override stays whatever the TOML resolved to.
    if let Ok(bytes) = std::env::var("SCITADEL_ACQUIRE_BODY_CAP_BYTES") {
        match bytes.parse::<u64>() {
            Ok(v) => {
                let candidate = AcquireConfig {
                    body_cap_bytes: Some(v),
                };
                if let Err(msg) = candidate.validate() {
                    tracing::warn!(
                        env = "SCITADEL_ACQUIRE_BODY_CAP_BYTES",
                        error = %msg,
                        "env override rejected; keeping the TOML/default value"
                    );
                } else {
                    config.acquire.body_cap_bytes = Some(v);
                }
            }
            Err(e) => tracing::warn!(
                value = %bytes,
                error = %e,
                "SCITADEL_ACQUIRE_BODY_CAP_BYTES is not a u64; keeping the default"
            ),
        }
    }

    config
}

/// Load config from a specific TOML file path.
///
/// Unlike [`load_config`], this is **fallible**: a TOML that parses but
/// violates [`AcquireConfig::validate`] (#288) returns
/// [`crate::error::CoreError::Config`] rather than being silently discarded —
/// a path-named load is the explicit "please tell me what's wrong" shape.
pub fn load_config_from(path: &Path) -> Result<Config, crate::error::CoreError> {
    let contents = std::fs::read_to_string(path)?;
    let mut config: Config =
        toml::from_str(&contents).map_err(|e| crate::error::CoreError::Config(e.to_string()))?;
    config.openalex.migrate_legacy_email();
    config
        .acquire
        .validate()
        .map_err(crate::error::CoreError::Config)?;
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

    /// #288: `[acquire] body_cap_bytes` flows through the TOML loader.
    /// A missing section keeps the workspace default (`None`); an explicit
    /// value is honoured as-is, so `PaperDownloader::with_body_cap_override`
    /// sees exactly what the operator asked for.
    #[test]
    fn body_cap_bytes_loads_from_toml_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "db_path = \"/tmp/x.db\"\n\n[acquire]\nbody_cap_bytes = 536870912\n",
        )
        .unwrap();

        let config = load_config_from(&path).unwrap();
        assert_eq!(config.acquire.body_cap_bytes, Some(536870912));
    }

    #[test]
    fn body_cap_bytes_is_none_when_the_section_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "db_path = \"/tmp/x.db\"\n").unwrap();

        let config = load_config_from(&path).unwrap();
        assert_eq!(config.acquire.body_cap_bytes, None);
    }

    // -----------------------------------------------------------------
    // #288: `body_cap_bytes` validation.
    //
    // A value of 0, or 100 meant as megabytes, would silently refuse every
    // real full-text PDF the ladder ever fetches (every body > cap → terminal
    // `too_large`). The loader rejects such values at load time instead of
    // discovering the mistake on the first fetch.
    // -----------------------------------------------------------------

    /// The infallible `validate` reports 0 as a config mistake, naming the
    /// key, the unit and the valid range.
    #[test]
    fn body_cap_bytes_of_zero_fails_validation() {
        let cfg = AcquireConfig {
            body_cap_bytes: Some(0),
        };
        let err = cfg
            .validate()
            .expect_err("zero bytes is below the 1 MiB floor");
        assert!(
            err.contains("acquire.body_cap_bytes"),
            "names the key: {err}"
        );
        assert!(
            err.contains("bytes"),
            "spells out the unit (bytes, not MB): {err}"
        );
        assert!(
            err.contains(&BODY_CAP_MIN_BYTES.to_string()),
            "names the floor of the valid range: {err}"
        );
    }

    /// The common mistake — `100` meant as megabytes. 100 bytes is below the
    /// floor, so validation rejects it with the same clear error.
    #[test]
    fn body_cap_bytes_of_one_hundred_fails_validation_as_below_the_floor() {
        let cfg = AcquireConfig {
            body_cap_bytes: Some(100),
        };
        let err = cfg
            .validate()
            .expect_err("100 bytes is below the 1 MiB floor (the MB-mistake case)");
        assert!(err.contains("100"), "quotes the bad value: {err}");
        assert!(
            err.contains("100") && err.contains("MB"),
            "names the MB mistake explicitly: {err}"
        );
    }

    /// A value at or above the floor passes.
    #[test]
    fn a_valid_body_cap_passes_validation() {
        for bytes in [BODY_CAP_MIN_BYTES, 100 * 1024 * 1024, 500 * 1024 * 1024] {
            AcquireConfig {
                body_cap_bytes: Some(bytes),
            }
            .validate()
            .unwrap_or_else(|e| panic!("{bytes} must pass: {e}"));
        }
    }

    /// `None` is the default and has no cap to validate.
    #[test]
    fn a_none_body_cap_passes_validation() {
        AcquireConfig::default().validate().unwrap();
    }

    /// `load_config_from` is the fallible loader, so a TOML with a sub-floor
    /// value is a `CoreError::Config` rather than a silent reset — the one
    /// path that lets a tool (or a test) say "please tell me what's wrong".
    #[test]
    fn load_config_from_rejects_a_sub_floor_body_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "db_path = \"/tmp/x.db\"\n\n[acquire]\nbody_cap_bytes = 0\n",
        )
        .unwrap();

        let err = load_config_from(&path).expect_err("a zero cap is a config error");
        let msg = err.to_string();
        assert!(
            msg.contains("acquire.body_cap_bytes"),
            "surfaces the key: {msg}"
        );
    }

    #[test]
    fn load_config_from_rejects_the_mb_mistake() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "db_path = \"/tmp/x.db\"\n\n[acquire]\nbody_cap_bytes = 100\n",
        )
        .unwrap();

        load_config_from(&path).expect_err("100 bytes is the MB-mistake case");
    }
}
