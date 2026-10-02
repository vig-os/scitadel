//! Credential resolution: secret store → environment variable → config file → None.
//!
//! The secret store is chosen at runtime from what the machine actually
//! offers ([`backend`]):
//!
//! * **macOS Keychain** — the login Keychain, via Security.framework directly.
//!   Not the `security` CLI: that takes the password as an argv value, so it
//!   is readable from `ps` by any local user (#248), and its source shows
//!   there is no stdin prompt to route around that. Calling the framework
//!   also keeps scitadel itself as the trusted application on the item, so
//!   reading a credential back does not raise an authorization prompt.
//! * **Secret Service** — libsecret via `secret-tool` (GNOME Keyring, KWallet,
//!   KeePassXC…), used on Linux/BSD when a Secret Service actually answers.
//!   The secret goes in on stdin, never on argv.
//! * **File** — a `0600` TOML file under the XDG config dir. The fallback for
//!   headless boxes, containers and CI, where no secret daemon is running.
//!
//! Each source has one or more named secrets, stored under service
//! "scitadel" with the credential key as the account name.
//!
//! Secret *values* never reach a log line, an error message, stdout or the
//! process table — only key names, backend names and the store path do.
//! Values are carried in [`Secret`], which redacts its own formatting,
//! zeroizes on drop, and does not implement `Serialize`.

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
    /// macOS login Keychain, via Security.framework directly.
    ///
    /// Exists only on macOS. The variant is compiled out elsewhere rather
    /// than left as an unhandled value, so `SCITADEL_CREDENTIAL_BACKEND=macos-keychain`
    /// on Linux degrades to auto-detection instead of selecting a backend
    /// with no implementation behind it.
    #[cfg(target_os = "macos")]
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
            #[cfg(target_os = "macos")]
            Self::MacosKeychain => "macos-keychain",
            Self::SecretService => "secret-service",
            Self::File => "file",
        }
    }

    /// One-line description for `auth status`, including where secrets live.
    pub fn describe(self) -> String {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacosKeychain => "macos-keychain (login keychain via Security.framework)".into(),
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

// ---------------------------------------------------------------------------
// Secret
// ---------------------------------------------------------------------------

/// A credential value, carried in a type that resists leaking.
///
/// Three properties, each of which has been a real way for a secret to
/// escape this module:
///
/// * [`std::fmt::Debug`] and [`std::fmt::Display`] print `<redacted>`, so a
///   secret in a struct field, a `{:?}` log line or an error message is
///   never spelled out. Nothing has to remember to redact it.
/// * The buffer is [`zeroize`]d on drop, so it does not linger in freed
///   heap that a later crash dump or `/proc/<pid>/mem` read could pick up.
/// * It deliberately does **not** implement `serde::Serialize`. A secret
///   cannot reach an HTTP request body, a config dump or a TUI snapshot
///   by accident; a caller that genuinely needs to send it has to convert
///   deliberately via [`Secret::expose`], which shows up in review.
///
/// Construction is explicit ([`Secret::new`]), and so is every read
/// ([`Secret::expose`]). The friction is the point: the small number of
/// sites that must touch the plaintext should be visible.
pub struct Secret(String);

impl Secret {
    /// Wrap a plaintext value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the plaintext.
    ///
    /// Named rather than left as a `Deref` so that reading a secret is a
    /// greppable act. Every call site is a place a value could be logged
    /// or serialised, so every call site is worth seeing.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the plaintext.
    ///
    /// The buffer is not zeroed on this path — ownership of the string
    /// moves to the caller, who now owns clearing it (or not). Prefer
    /// [`Secret::expose`] where the value is only read.
    pub fn into_string(mut self) -> String {
        std::mem::take(&mut self.0)
    }

    /// Is the wrapped value empty after trimming? An empty credential is
    /// treated as absent by [`get`], matching the env-var and store paths.
    pub(crate) fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

fn parse_backend(name: &str) -> Option<Backend> {
    match name.trim().to_ascii_lowercase().as_str() {
        #[cfg(target_os = "macos")]
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
/// ignored rather than fatal, so a typo degrades to auto-detection. That
/// also covers `macos-keychain` on a non-macOS host, where the variant is
/// compiled out and so never parses.
///
/// The Keychain needs no reachability probe: it is either the platform
/// default or it is not, so only the Secret Service has to be asked.
fn select_backend(forced: Option<&str>, has_secret_service: bool) -> Backend {
    if let Some(name) = forced
        && let Some(b) = parse_backend(name)
    {
        return b;
    }
    if cfg!(target_os = "macos") {
        #[cfg(target_os = "macos")]
        return Backend::MacosKeychain;
    }
    if has_secret_service {
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
        select_backend(forced.as_deref(), secret_service_available())
    })
}

/// Is `name` an executable on `$PATH`?
fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
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
pub fn resolve(store_key: &str, env_var: &str, config_fallback: &str) -> Option<Secret> {
    // 1. Secret store (Keychain / Secret Service / 0600 file)
    if let Some(val) = get(store_key) {
        return Some(val);
    }

    // 2. Environment variable
    if let Ok(val) = std::env::var(env_var)
        && !val.trim().is_empty()
    {
        return Some(Secret(val));
    }

    // 3. Config fallback
    if !config_fallback.trim().is_empty() {
        return Some(Secret(config_fallback.to_string()));
    }

    None
}

/// Read a credential from the active secret store.
pub fn get(key: &str) -> Option<Secret> {
    let value = match backend() {
        #[cfg(target_os = "macos")]
        Backend::MacosKeychain => keychain_get(key),
        Backend::SecretService => secret_tool_get(key),
        Backend::File => file_get(key),
    }?;
    if value.is_blank() { None } else { Some(value) }
}

/// Store a credential in the active secret store.
pub fn store(key: &str, value: &Secret) -> Result<(), String> {
    match backend() {
        #[cfg(target_os = "macos")]
        Backend::MacosKeychain => keychain_store(key, value),
        Backend::SecretService => secret_tool_store(key, value),
        Backend::File => file_store(key, value),
    }
}

/// Delete a credential from the active secret store.
pub fn delete(key: &str) -> Result<(), String> {
    match backend() {
        #[cfg(target_os = "macos")]
        Backend::MacosKeychain => keychain_delete(key),
        Backend::SecretService => secret_tool_delete(key),
        Backend::File => file_delete(key),
    }
}

/// Deprecated alias for [`get`], kept so external callers don't break.
#[deprecated(note = "the store is no longer macOS-only; use `credentials::get`")]
pub fn get_keychain(key: &str) -> Option<Secret> {
    get(key)
}

// ---------------------------------------------------------------------------
// macOS Keychain backend
// ---------------------------------------------------------------------------
//
// Security.framework is called directly rather than through the `security`
// CLI. The CLI's `add-generic-password` takes the password as an argv
// value (`-w <password>`), which sits in the process table for the lifetime
// of the command and is readable by any local user with `ps` — #248.
//
// The obvious workaround, passing a bare `-w` so `security` prompts and
// reading the value from a pipe, does not work. Apple's SecurityTool
// (`keychain_add.c`) parses with `getopt(argc, argv, "a:c:C:D:G:j:l:s:p:w:UAT:")`
// and assigns `passwordData = optarg` for `-w`/`-p` with no prompt fallback.
// A bare trailing `-w` stores an *empty* password; `-w -U` consumes `-U` as
// the password. Only the framework API keeps the value out of argv.
//
// Items are written with `SecItemAdd`, the same modern API the `keyring`
// crate uses, so an item written by an older scitadel (via `security`) is
// still found by service + account. One consequence for existing users: an
// item written by the old code carries an ACL trusting `/usr/bin/security`,
// so the first read by scitadel itself may raise a one-time Keychain
// authorization prompt. Re-running `scitadel auth login <source>` rewrites
// the item with an ACL that trusts scitadel and clears the prompt.

/// `errSecItemNotFound` — the Keychain has no such item. Treated as "no
/// credential", never as an error.
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
fn keychain_get(key: &str) -> Option<Secret> {
    use security_framework::passwords::{PasswordOptions, generic_password};
    use zeroize::Zeroize;

    match generic_password(PasswordOptions::new_generic_password(SERVICE, key)) {
        // Wipe the Keychain's plaintext copy before the `Vec` is dropped.
        Ok(mut bytes) => {
            let value = Secret(String::from_utf8_lossy(&bytes).into_owned());
            bytes.zeroize();
            Some(value)
        }
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => None,
        // Any other status (locked keychain, denied access) reads as
        // absent rather than failing a search — same contract the CLI
        // path had, where a non-zero exit was a miss.
        Err(_) => None,
    }
}

#[cfg(target_os = "macos")]
fn keychain_store(key: &str, value: &Secret) -> Result<(), String> {
    use security_framework::passwords::set_generic_password;

    // Creates or updates in place, so unlike the CLI path there is no
    // delete-first dance to work around `add-generic-password`'s refusal
    // to touch an existing item.
    set_generic_password(SERVICE, key, value.expose().as_bytes())
        .map_err(|e| format!("failed to store credential '{key}' in the Keychain: {e}"))
}

#[cfg(target_os = "macos")]
fn keychain_delete(key: &str) -> Result<(), String> {
    use security_framework::passwords::delete_generic_password;

    match delete_generic_password(SERVICE, key) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => {
            Err(format!("credential '{key}' not found"))
        }
        Err(e) => Err(format!("failed to delete credential '{key}': {e}")),
    }
}

// ---------------------------------------------------------------------------
// Secret Service (`secret-tool`) backend
// ---------------------------------------------------------------------------

fn secret_tool_get(key: &str) -> Option<Secret> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", key])
        .output()
        .ok()?;
    if output.status.success() {
        Some(Secret(String::from_utf8_lossy(&output.stdout).into_owned()))
    } else {
        None
    }
}

fn secret_tool_store(key: &str, value: &Secret) -> Result<(), String> {
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
        .write_all(value.expose().as_bytes())
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

fn file_get_at(path: &std::path::Path, key: &str) -> Option<Secret> {
    file_read_all(path).get(key).map(Secret::new)
}

fn file_store_at(path: &std::path::Path, key: &str, value: &Secret) -> Result<(), String> {
    let mut entries = file_read_all(path);
    entries.insert(key.to_string(), value.expose().to_string());
    file_write_all(path, &entries)
}

fn file_delete_at(path: &std::path::Path, key: &str) -> Result<(), String> {
    let mut entries = file_read_all(path);
    if entries.remove(key).is_none() {
        return Err(format!("credential '{key}' not found"));
    }
    file_write_all(path, &entries)
}

fn file_get(key: &str) -> Option<Secret> {
    file_get_at(&file_store_path(), key)
}

fn file_store(key: &str, value: &Secret) -> Result<(), String> {
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
    } else if std::env::var(key.env_var).is_ok_and(|v| !v.is_empty()) {
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
            #[cfg(target_os = "macos")]
            Backend::MacosKeychain,
            Backend::SecretService,
            Backend::File,
        ] {
            assert_eq!(parse_backend(b.label()), Some(b));
        }
    }

    #[test]
    fn parse_backend_accepts_aliases_and_rejects_junk() {
        assert_eq!(parse_backend(" libsecret "), Some(Backend::SecretService));
        assert_eq!(parse_backend("FILE"), Some(Backend::File));
        assert_eq!(parse_backend("gnome-keyring"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_wins_on_macos_and_the_name_still_parses() {
        assert_eq!(parse_backend("Keychain"), Some(Backend::MacosKeychain));
        assert_eq!(
            parse_backend("macos-keychain"),
            Some(Backend::MacosKeychain)
        );
        assert_eq!(
            select_backend(None, true),
            Backend::MacosKeychain,
            "the Keychain is the macOS default and needs no reachability probe"
        );
        assert_eq!(
            select_backend(Some("file"), true),
            Backend::File,
            "an explicit override still wins"
        );
    }

    /// The Keychain backend is compiled out off macOS, so a shared config
    /// or runbook naming it must degrade to auto-detection rather than
    /// select a backend with no implementation behind it.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn macos_keychain_name_does_not_select_a_backend_off_macos() {
        assert_eq!(parse_backend("Keychain"), None);
        assert_eq!(parse_backend("macos-keychain"), None);
        assert_eq!(
            select_backend(Some("macos-keychain"), true),
            Backend::SecretService,
            "an unusable forced backend falls through to detection"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn selection_prefers_secret_service_then_file() {
        assert_eq!(
            select_backend(None, true),
            Backend::SecretService,
            "libsecret is the Linux default when a Secret Service answers"
        );
        assert_eq!(
            select_backend(None, false),
            Backend::File,
            "headless boxes fall back to the 0600 file store"
        );
    }

    #[test]
    fn env_override_wins_over_detection() {
        assert_eq!(select_backend(Some("file"), true), Backend::File);
        assert_eq!(
            select_backend(Some("secret-service"), false),
            Backend::SecretService
        );
    }

    #[test]
    fn unrecognised_env_override_falls_back_to_detection() {
        // What detection returns is platform-dependent: the Keychain is the
        // macOS default whether or not a Secret Service answers. Asserting
        // the Linux answer unconditionally failed on the macos runner.
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                select_backend(Some("nonsense"), true),
                Backend::MacosKeychain
            );
            assert_eq!(select_backend(Some(""), false), Backend::MacosKeychain);
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(
                select_backend(Some("nonsense"), true),
                Backend::SecretService
            );
            assert_eq!(select_backend(Some(""), false), Backend::File);
        }
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

        assert_eq!(
            file_get_at(&path, "openalex.api_key")
                .as_ref()
                .map(Secret::expose),
            None
        );
        file_store_at(&path, "openalex.api_key", &Secret::new("s3cret")).unwrap();
        file_store_at(&path, "openalex.email", &Secret::new("me@example.org")).unwrap();

        assert_eq!(
            file_get_at(&path, "openalex.api_key")
                .as_ref()
                .map(Secret::expose),
            Some("s3cret")
        );
        assert_eq!(
            file_get_at(&path, "openalex.email")
                .as_ref()
                .map(Secret::expose),
            Some("me@example.org"),
            "storing a second key must not clobber the first"
        );

        file_delete_at(&path, "openalex.api_key").unwrap();
        assert_eq!(
            file_get_at(&path, "openalex.api_key")
                .as_ref()
                .map(Secret::expose),
            None
        );
        assert_eq!(
            file_get_at(&path, "openalex.email")
                .as_ref()
                .map(Secret::expose),
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
        file_store_at(&path, "lens.api_token", &Secret::new("tok")).unwrap();

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
        assert_eq!(
            file_get_at(&path, "pubmed.api_key")
                .as_ref()
                .map(Secret::expose),
            None
        );
        file_store_at(&path, "pubmed.api_key", &Secret::new("abc")).unwrap();
        assert_eq!(
            file_get_at(&path, "pubmed.api_key")
                .as_ref()
                .map(Secret::expose),
            Some("abc")
        );
    }

    // -----------------------------------------------------------------------
    // Secret (#248)
    // -----------------------------------------------------------------------

    /// #248: a secret must never be spellable by accident. `Debug` reaches
    /// struct fields, log lines and `unwrap()` panic output; `Display`
    /// reaches `{}` interpolation. Both have to redact.
    // Redaction must survive nesting, which is where `Debug` leaks actually
    // happen — a secret sitting in a config struct, not a bare value.
    #[derive(Debug)]
    #[allow(dead_code)]
    struct NestedHolder {
        api_key: Secret,
    }

    #[test]
    fn secret_redacts_in_debug_and_display() {
        let s = Secret::new("hunter2-correct-horse");

        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
        assert_eq!(format!("{s}"), "<redacted>");
        assert!(
            !format!("{s:?} {s}").contains("hunter2"),
            "the plaintext must not appear in either formatter"
        );

        let rendered = format!(
            "{:?}",
            NestedHolder {
                api_key: Secret::new("sk-live-abcdef"),
            }
        );
        assert!(
            !rendered.contains("sk-live"),
            "a derived Debug impl leaked the value: {rendered}"
        );

        // ...and with a trailing newline, which is what a `tracing` field or
        // a log line actually produces.
        assert!(
            !format!(
                "{:?}
",
                Secret::new("sk-live-abcdef")
            )
            .contains("sk-live")
        );
    }

    /// The plaintext is reachable only through the explicitly named
    /// `expose`, which is what keeps every read site greppable.
    #[test]
    fn secret_exposes_its_value_on_request() {
        let s = Secret::new("  padded-value  ");
        assert_eq!(s.expose(), "  padded-value  ");
        assert_eq!(s.into_string(), "  padded-value  ");
        assert!(Secret::new("   ").is_blank());
        assert!(!Secret::new(" x ").is_blank());
    }

    /// A blank value must read as absent, so a whitespace-only credential
    /// cannot shadow a real one or produce an auth attempt with no key.
    #[test]
    fn blank_secrets_are_treated_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");

        file_store_at(&path, "openalex.api_key", &Secret::new("   \n  ")).unwrap();
        assert!(
            file_get_at(&path, "openalex.api_key").is_some(),
            "the file backend stores what it is given"
        );
        // `get` is the layer that applies the blank rule; it needs a
        // process-global backend, so exercise the rule it uses directly.
        assert!(Secret::new("   \n  ").is_blank());
    }

    /// #248: the argv property, tested where it can actually run.
    ///
    /// The macOS store path no longer spawns anything — it calls
    /// Security.framework directly, because the `security` CLI takes the
    /// password as an argv value and its source shows there is no stdin
    /// prompt to fall back on. This test pins the same invariant on the
    /// one store path that still shells out, using a fake `secret-tool`
    /// that records both its argv and its stdin.
    ///
    /// It discriminates: passing the secret as an argv value instead of
    /// writing it to stdin fails the `argv` assertion outright.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn secret_tool_store_keeps_the_secret_out_of_argv() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let argv_log = dir.path().join("argv.txt");
        let stdin_log = dir.path().join("stdin.txt");

        // A stand-in for `secret-tool` that records exactly what it was
        // handed, then succeeds.
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > "{argv}"
cat > "{stdin}"
"#,
            argv = argv_log.display(),
            stdin = stdin_log.display(),
        );
        let fake = bin.join("secret-tool");
        std::fs::write(&fake, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let secret = "argv-canary-2f8c1d";
        // Prepend rather than replace, so the fake can still find `cat`.
        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = {
            let mut parts = vec![bin.clone()];
            parts.extend(std::env::split_paths(&old_path));
            std::env::join_paths(parts).expect("PATH must stay valid")
        };
        // SAFETY: the body is single-threaded and nothing else in this
        // process spawns concurrently.
        unsafe { std::env::set_var("PATH", &new_path) };
        let result = secret_tool_store("openalex.api_key", &Secret::new(secret));
        unsafe { std::env::set_var("PATH", old_path) };
        result.expect("the fake secret-tool exits 0");

        let argv = std::fs::read_to_string(&argv_log).unwrap();
        let stdin = std::fs::read_to_string(&stdin_log).unwrap();

        assert!(
            !argv.contains(secret),
            "the secret reached the process argv, readable via ps: {argv:?}"
        );
        assert_eq!(
            stdin.trim_end(),
            secret,
            "the secret must arrive on stdin instead"
        );
        assert!(
            argv.contains("openalex.api_key"),
            "the key name is not secret and belongs in argv: {argv:?}"
        );
    }

    /// #248: the macOS store path is a real Keychain round trip. It needs an
    /// end-to-end check because the path changed, and because the failure
    /// mode the old code was one edit away from — an empty password
    /// reaching the Keychain — is silent.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_keychain_round_trips_and_never_stores_empty() {
        let key = "test.roundtrip";
        let _ = keychain_delete(key);

        // Characters a bare `-w` on the CLI could not have carried intact,
        // so a mangled write is distinguishable from a correct one.
        let secret = "not empty: spaces, 'quotes', and (parens)";

        keychain_store(key, &Secret::new(secret)).unwrap();
        let read = keychain_get(key).expect("value must be readable after store");
        assert_eq!(
            read.expose(),
            secret,
            "an empty or mangled read means the value never reached the Keychain"
        );

        // Storing again must update in place rather than fail or duplicate.
        keychain_store(key, &Secret::new("second")).unwrap();
        assert_eq!(keychain_get(key).unwrap().expose(), "second");

        keychain_delete(key).unwrap();
        assert!(
            keychain_get(key).is_none(),
            "a deleted item must read as absent, not as an error"
        );
        assert!(
            keychain_delete(key).is_err(),
            "deleting an absent item is an error, matching the file backend"
        );
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
