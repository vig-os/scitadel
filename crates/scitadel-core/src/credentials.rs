//! Credential resolution: secret store → environment variable → config file → None.
//!
//! The secret store is chosen at runtime from what the machine actually
//! offers ([`backend`]):
//!
//! * **macOS keychain** — the login Keychain via the `security` CLI. Shelling
//!   out avoids the per-binary authorization prompts the `keyring` crate
//!   triggers.
//! * **Secret Service** — libsecret via `secret-tool` (GNOME Keyring, KWallet,
//!   KeePassXC…), used on Linux/BSD when a Secret Service actually answers.
//! * **File** — a `0600` TOML file under the XDG config dir. The fallback for
//!   headless boxes, containers and CI, where no secret daemon is running.
//!
//! Each source has one or more named secrets, stored under service
//! "scitadel" with the credential key as the account name.
//!
//! Secret *values* never reach a log line, an error message or stdout —
//! only key names, backend names and the store path do.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

const SERVICE: &str = "scitadel";

/// Force a specific backend (`macos-keychain`, `secret-service`, `file`).
/// Escape hatch for headless sessions and for tests.
const BACKEND_ENV: &str = "SCITADEL_CREDENTIAL_BACKEND";

/// Override the file-backend location (absolute path). Used by tests.
const FILE_ENV: &str = "SCITADEL_CREDENTIALS_FILE";

/// Account name used to probe whether a Secret Service is reachable.
/// Never stored; the lookup is expected to miss.
const PROBE_ACCOUNT: &str = "__scitadel_probe__";

/// A credential that was not found, with instructions on how to set it.
#[derive(Debug)]
pub struct MissingCredential {
    pub source: String,
    pub keys: Vec<String>,
    pub env_vars: Vec<String>,
    pub remedy: String,
}

impl std::fmt::Display for MissingCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{source} credentials not configured.\n\n\
             To authenticate, run:\n  scitadel auth login {source}\n\n\
             Or set environment variable(s):\n{env_hint}",
            source = self.source,
            env_hint = self
                .env_vars
                .iter()
                .map(|v| format!("  {v}=<value>"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

/// The secret store scitadel talks to on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// macOS login Keychain via the `security` CLI.
    MacosKeychain,
    /// Freedesktop Secret Service via the `secret-tool` CLI.
    SecretService,
    /// `0600` TOML file under the XDG config dir.
    File,
}

impl Backend {
    /// Stable machine-readable name, also accepted by [`BACKEND_ENV`].
    pub fn label(self) -> &'static str {
        match self {
            Self::MacosKeychain => "macos-keychain",
            Self::SecretService => "secret-service",
            Self::File => "file",
        }
    }

    /// One-line description for `auth status`, including where secrets live.
    pub fn describe(self) -> String {
        match self {
            Self::MacosKeychain => "macos-keychain (login keychain via `security`)".into(),
            Self::SecretService => "secret-service (libsecret via `secret-tool`)".into(),
            Self::File => format!("file ({}, mode 0600)", file_store_path().display()),
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

fn parse_backend(name: &str) -> Option<Backend> {
    match name.trim().to_ascii_lowercase().as_str() {
        "macos-keychain" | "macos" | "keychain" | "security" => Some(Backend::MacosKeychain),
        "secret-service" | "secretservice" | "libsecret" | "secret-tool" => {
            Some(Backend::SecretService)
        }
        "file" | "plaintext" => Some(Backend::File),
        _ => None,
    }
}

/// Pure backend-selection policy, split out from the probing so it can be
/// unit-tested without a Keychain, a D-Bus session or a `$HOME`.
///
/// `forced` is the raw [`BACKEND_ENV`] value; an unrecognised one is
/// ignored rather than fatal, so a typo degrades to auto-detection.
fn select_backend(forced: Option<&str>, has_security: bool, has_secret_service: bool) -> Backend {
    if let Some(name) = forced
        && let Some(b) = parse_backend(name)
    {
        return b;
    }
    if has_security {
        Backend::MacosKeychain
    } else if has_secret_service {
        Backend::SecretService
    } else {
        Backend::File
    }
}

/// The backend in use for this process. Detected once and cached: probing
/// spawns subprocesses, and a store that appears mid-run would split
/// secrets across two backends.
pub fn backend() -> Backend {
    static CACHED: OnceLock<Backend> = OnceLock::new();
    *CACHED.get_or_init(|| {
        let forced = std::env::var(BACKEND_ENV).ok();
        select_backend(
            forced.as_deref(),
            macos_security_available(),
            secret_service_available(),
        )
    })
}

/// Is `name` an executable on `$PATH`?
fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

fn macos_security_available() -> bool {
    cfg!(target_os = "macos") && binary_on_path("security")
}

/// `secret-tool` on `$PATH` *and* a Secret Service that answers.
///
/// The probe looks up an account that is never stored. A reachable
/// service reports the miss with a non-zero exit and silent stderr; with
/// no D-Bus session or no keyring daemon, `secret-tool` writes a
/// diagnostic to stderr instead — that's the signal to fall back to the
/// file store rather than fail every `auth login` (#212).
fn secret_service_available() -> bool {
    if !binary_on_path("secret-tool") {
        return false;
    }
    Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", PROBE_ACCOUNT])
        .output()
        .is_ok_and(|out| out.stderr.is_empty())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get a credential value by trying the secret store, then env var, then
/// the config-file fallback.
///
/// Returns the first non-empty value found, or `None`.
pub fn resolve(store_key: &str, env_var: &str, config_fallback: &str) -> Option<String> {
    // 1. Secret store (keychain / Secret Service / 0600 file)
    if let Some(val) = get(store_key) {
        return Some(val);
    }

    // 2. Environment variable
    if let Ok(val) = std::env::var(env_var)
        && !val.is_empty()
    {
        return Some(val);
    }

    // 3. Config fallback
    if !config_fallback.is_empty() {
        return Some(config_fallback.to_string());
    }

    None
}

/// Read a credential from the active secret store.
pub fn get(key: &str) -> Option<String> {
    let value = match backend() {
        Backend::MacosKeychain => security_get(key),
        Backend::SecretService => secret_tool_get(key),
        Backend::File => file_get(key),
    }?;
    let value = value.trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// Store a credential in the active secret store.
pub fn store(key: &str, value: &str) -> Result<(), String> {
    match backend() {
        Backend::MacosKeychain => security_store(key, value),
        Backend::SecretService => secret_tool_store(key, value),
        Backend::File => file_store(key, value),
    }
}

/// Delete a credential from the active secret store.
pub fn delete(key: &str) -> Result<(), String> {
    match backend() {
        Backend::MacosKeychain => security_delete(key),
        Backend::SecretService => secret_tool_delete(key),
        Backend::File => file_delete(key),
    }
}

/// Deprecated alias for [`get`], kept so external callers don't break.
#[deprecated(note = "the store is no longer macOS-only; use `credentials::get`")]
pub fn get_keychain(key: &str) -> Option<String> {
    get(key)
}

// ---------------------------------------------------------------------------
// macOS `security` backend
// ---------------------------------------------------------------------------

fn security_get(key: &str) -> Option<String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", SERVICE, "-a", key, "-w"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

fn security_store(key: &str, value: &str) -> Result<(), String> {
    // Delete first: `add-generic-password` fails on an existing entry.
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", key])
        .output();

    let output = Command::new("security")
        .args([
            "add-generic-password",
            "-s",
            SERVICE,
            "-a",
            key,
            "-w",
            value,
            "-U", // update if exists
        ])
        .output()
        .map_err(|e| format!("failed to run security CLI: {e}"))?;

    check_status(&output, key, "store")
}

fn security_delete(key: &str) -> Result<(), String> {
    let output = Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", key])
        .output()
        .map_err(|e| format!("failed to run security CLI: {e}"))?;

    check_status(&output, key, "delete")
}

// ---------------------------------------------------------------------------
// Secret Service (`secret-tool`) backend
// ---------------------------------------------------------------------------

fn secret_tool_get(key: &str) -> Option<String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", key])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

fn secret_tool_store(key: &str, value: &str) -> Result<(), String> {
    use std::io::Write;

    // `secret-tool store` reads the secret from stdin, which keeps it off
    // the process's argv (and so out of `ps` output and shell history).
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label",
            &format!("scitadel: {key}"),
            "service",
            SERVICE,
            "account",
            key,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run secret-tool: {e}"))?;

    child
        .stdin
        .take()
        .ok_or_else(|| "failed to open secret-tool stdin".to_string())?
        .write_all(value.as_bytes())
        .map_err(|e| format!("failed to write credential to secret-tool: {e}"))?;

    let output = child
        .wait_with_output()
        .map_err(|e| format!("secret-tool did not complete: {e}"))?;

    check_status(&output, key, "store")
}

fn secret_tool_delete(key: &str) -> Result<(), String> {
    let output = Command::new("secret-tool")
        .args(["clear", "service", SERVICE, "account", key])
        .output()
        .map_err(|e| format!("failed to run secret-tool: {e}"))?;

    check_status(&output, key, "delete")
}

fn check_status(output: &std::process::Output, key: &str, verb: &str) -> Result<(), String> {
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "failed to {verb} credential '{key}': {}",
            stderr.trim()
        ))
    }
}

// ---------------------------------------------------------------------------
// File backend
// ---------------------------------------------------------------------------

/// Where the file backend keeps its `0600` TOML store.
///
/// `SCITADEL_CREDENTIALS_FILE` > `$XDG_CONFIG_HOME/scitadel/credentials.toml`
/// > `$HOME/.config/scitadel/credentials.toml`.
pub fn file_store_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os(FILE_ENV) {
        return PathBuf::from(explicit);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("scitadel").join("credentials.toml")
}

fn file_read_all(path: &std::path::Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|c| toml::from_str(&c).ok())
        .unwrap_or_default()
}

fn file_write_all(
    path: &std::path::Path,
    entries: &BTreeMap<String, String>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("credential store path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    restrict_dir(parent);

    let body = toml::to_string_pretty(entries)
        .map_err(|e| format!("failed to serialise credential store: {e}"))?;
    let contents = format!(
        "# scitadel credential store — written by `scitadel auth login`.\n\
         # Plain text, mode 0600: the fallback used when no OS secret\n\
         # service is available. Do not commit this file.\n\n{body}"
    );

    // Write to a sibling temp file created 0600 up front, then rename, so
    // the secrets are never readable by anyone else even momentarily.
    let tmp = path.with_extension("toml.tmp");
    write_private(&tmp, &contents)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("failed to write {}: {e}", path.display())
    })
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, contents: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("failed to create {}: {e}", path.display()))?;
    f.write_all(contents.as_bytes())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

#[cfg(unix)]
fn restrict_dir(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &std::path::Path) {}

fn file_get_at(path: &std::path::Path, key: &str) -> Option<String> {
    file_read_all(path).get(key).cloned()
}

fn file_store_at(path: &std::path::Path, key: &str, value: &str) -> Result<(), String> {
    let mut entries = file_read_all(path);
    entries.insert(key.to_string(), value.to_string());
    file_write_all(path, &entries)
}

fn file_delete_at(path: &std::path::Path, key: &str) -> Result<(), String> {
    let mut entries = file_read_all(path);
    if entries.remove(key).is_none() {
        return Err(format!("credential '{key}' not found"));
    }
    file_write_all(path, &entries)
}

fn file_get(key: &str) -> Option<String> {
    file_get_at(&file_store_path(), key)
}

fn file_store(key: &str, value: &str) -> Result<(), String> {
    file_store_at(&file_store_path(), key, value)
}

fn file_delete(key: &str) -> Result<(), String> {
    file_delete_at(&file_store_path(), key)
}

// ---------------------------------------------------------------------------
// Source credential definitions
// ---------------------------------------------------------------------------

/// Definitions of credentials required by each source.
pub struct SourceCredentials {
    pub source: &'static str,
    pub keys: &'static [CredentialKey],
}

pub struct CredentialKey {
    /// Account name in the secret store, e.g. `openalex.api_key`.
    pub store_key: &'static str,
    pub env_var: &'static str,
    pub label: &'static str,
    /// Read without echo when prompting, and never printed back.
    pub secret: bool,
    /// The source still works without it, so its absence must not make
    /// the whole source read as "not configured".
    pub optional: bool,
}

pub static PATENTSVIEW_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "patentsview",
    keys: &[CredentialKey {
        store_key: "patentsview.api_key",
        env_var: "SCITADEL_PATENTSVIEW_KEY",
        label: "API key",
        secret: true,
        optional: false,
    }],
};

pub static PUBMED_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "pubmed",
    keys: &[CredentialKey {
        store_key: "pubmed.api_key",
        env_var: "SCITADEL_PUBMED_API_KEY",
        label: "API key",
        secret: true,
        optional: false,
    }],
};

/// OpenAlex takes two credentials: the metered API key it now requires
/// once the shared per-IP budget is spent, plus the polite-pool contact
/// address (optional, and not a secret) (#212).
pub static OPENALEX_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "openalex",
    keys: &[
        CredentialKey {
            store_key: "openalex.api_key",
            env_var: "SCITADEL_OPENALEX_API_KEY",
            label: "API key",
            secret: true,
            optional: false,
        },
        CredentialKey {
            store_key: "openalex.email",
            env_var: "SCITADEL_OPENALEX_EMAIL",
            label: "Email (polite pool, optional)",
            secret: false,
            optional: true,
        },
    ],
};

pub static LENS_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "lens",
    keys: &[CredentialKey {
        store_key: "lens.api_token",
        env_var: "SCITADEL_LENS_TOKEN",
        label: "API token",
        secret: true,
        optional: false,
    }],
};

pub static EPO_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "epo",
    keys: &[
        CredentialKey {
            store_key: "epo.consumer_key",
            env_var: "SCITADEL_EPO_KEY",
            label: "Consumer key",
            secret: false,
            optional: false,
        },
        CredentialKey {
            store_key: "epo.consumer_secret",
            env_var: "SCITADEL_EPO_SECRET",
            label: "Consumer secret",
            secret: true,
            optional: false,
        },
    ],
};

/// All sources that support authentication.
pub static ALL_SOURCES: &[&SourceCredentials] = &[
    &PUBMED_CREDENTIALS,
    &OPENALEX_CREDENTIALS,
    &PATENTSVIEW_CREDENTIALS,
    &LENS_CREDENTIALS,
    &EPO_CREDENTIALS,
];

/// Check whether a source has all its *required* credentials configured.
pub fn check_source(creds: &SourceCredentials) -> Result<(), MissingCredential> {
    let missing: Vec<&CredentialKey> = creds
        .keys
        .iter()
        .filter(|k| !k.optional && resolve(k.store_key, k.env_var, "").is_none())
        .collect();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(MissingCredential {
            source: creds.source.to_string(),
            keys: missing.iter().map(|k| k.store_key.to_string()).collect(),
            env_vars: missing.iter().map(|k| k.env_var.to_string()).collect(),
            remedy: format!("scitadel auth login {}", creds.source),
        })
    }
}

/// Where a single credential is currently coming from, for `auth status`.
/// Returns the backend label, `"env"`, or `"missing"` — never the value.
pub fn key_location(key: &CredentialKey) -> String {
    if get(key.store_key).is_some() {
        backend().label().to_string()
    } else if std::env::var(key.env_var)
        .ok()
        .is_some_and(|v| !v.is_empty())
    {
        "env".to_string()
    } else {
        "missing".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_labels_round_trip_through_parse() {
        for b in [
            Backend::MacosKeychain,
            Backend::SecretService,
            Backend::File,
        ] {
            assert_eq!(parse_backend(b.label()), Some(b));
        }
    }

    #[test]
    fn parse_backend_accepts_aliases_and_rejects_junk() {
        assert_eq!(parse_backend("Keychain"), Some(Backend::MacosKeychain));
        assert_eq!(parse_backend(" libsecret "), Some(Backend::SecretService));
        assert_eq!(parse_backend("FILE"), Some(Backend::File));
        assert_eq!(parse_backend("gnome-keyring"), None);
    }

    #[test]
    fn selection_prefers_macos_then_secret_service_then_file() {
        assert_eq!(
            select_backend(None, true, true),
            Backend::MacosKeychain,
            "macOS keychain wins when `security` is present"
        );
        assert_eq!(
            select_backend(None, false, true),
            Backend::SecretService,
            "libsecret is the Linux default when a Secret Service answers"
        );
        assert_eq!(
            select_backend(None, false, false),
            Backend::File,
            "headless boxes fall back to the 0600 file store"
        );
    }

    #[test]
    fn env_override_wins_over_detection() {
        assert_eq!(select_backend(Some("file"), true, true), Backend::File);
        assert_eq!(
            select_backend(Some("secret-service"), true, false),
            Backend::SecretService
        );
    }

    #[test]
    fn unrecognised_env_override_falls_back_to_detection() {
        assert_eq!(
            select_backend(Some("nonsense"), false, true),
            Backend::SecretService
        );
        assert_eq!(select_backend(Some(""), false, false), Backend::File);
    }

    #[test]
    fn openalex_needs_the_key_but_not_the_email() {
        let keys = OPENALEX_CREDENTIALS.keys;
        let api_key = keys.iter().find(|k| k.store_key == "openalex.api_key");
        let email = keys.iter().find(|k| k.store_key == "openalex.email");

        let api_key = api_key.expect("openalex.api_key credential must exist");
        assert_eq!(api_key.env_var, "SCITADEL_OPENALEX_API_KEY");
        assert!(api_key.secret, "an API key must be read without echo");
        assert!(!api_key.optional);

        let email = email.expect("openalex.email credential must exist");
        assert_eq!(email.env_var, "SCITADEL_OPENALEX_EMAIL");
        assert!(!email.secret, "the polite-pool address is not a secret");
        assert!(email.optional, "OpenAlex works without a mailto");
    }

    #[test]
    fn file_backend_round_trips_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("credentials.toml");

        assert_eq!(file_get_at(&path, "openalex.api_key"), None);
        file_store_at(&path, "openalex.api_key", "s3cret").unwrap();
        file_store_at(&path, "openalex.email", "me@example.org").unwrap();

        assert_eq!(
            file_get_at(&path, "openalex.api_key").as_deref(),
            Some("s3cret")
        );
        assert_eq!(
            file_get_at(&path, "openalex.email").as_deref(),
            Some("me@example.org"),
            "storing a second key must not clobber the first"
        );

        file_delete_at(&path, "openalex.api_key").unwrap();
        assert_eq!(file_get_at(&path, "openalex.api_key"), None);
        assert_eq!(
            file_get_at(&path, "openalex.email").as_deref(),
            Some("me@example.org")
        );

        assert!(
            file_delete_at(&path, "openalex.api_key").is_err(),
            "deleting an absent credential is an error, not a silent no-op"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_backend_store_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scitadel").join("credentials.toml");
        file_store_at(&path, "lens.api_token", "tok").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "credential file must not be group/world readable"
        );

        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "credential dir must not be group/world readable"
        );
    }

    #[test]
    fn file_backend_survives_a_corrupt_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        std::fs::write(&path, "this is not = valid = toml").unwrap();

        // A garbled file reads as empty rather than panicking, and the
        // next write repairs it.
        assert_eq!(file_get_at(&path, "pubmed.api_key"), None);
        file_store_at(&path, "pubmed.api_key", "abc").unwrap();
        assert_eq!(file_get_at(&path, "pubmed.api_key").as_deref(), Some("abc"));
    }

    #[test]
    fn every_source_key_is_uniquely_named() {
        let mut seen = std::collections::HashSet::new();
        for source in ALL_SOURCES {
            for key in source.keys {
                assert!(
                    seen.insert(key.store_key),
                    "duplicate store key {}",
                    key.store_key
                );
            }
        }
    }
}
