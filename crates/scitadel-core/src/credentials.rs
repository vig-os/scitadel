//! Credential resolution: keychain → environment variable → config file → None.
//!
//! Credentials are stored in the macOS Keychain via the `security` CLI tool,
//! which avoids per-binary authorization prompts that the `keyring` crate triggers.
//! Each source has one or more named secrets stored as generic passwords
//! with service "scitadel" and the key as the account name.

const SERVICE: &str = "scitadel";

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

/// Get a credential value by trying keychain, then env var, then config fallback.
///
/// Returns the first non-empty value found, or `None`.
pub fn resolve(keychain_key: &str, env_var: &str, config_fallback: &str) -> Option<String> {
    // 1. Keychain (via security CLI — no per-binary auth prompts)
    if let Some(val) = get_keychain(keychain_key) {
        return Some(val);
    }

    // 2. Environment variable
    if let Ok(val) = std::env::var(env_var) {
        if !val.is_empty() {
            return Some(val);
        }
    }

    // 3. Config fallback
    if !config_fallback.is_empty() {
        return Some(config_fallback.to_string());
    }

    None
}

/// Store a credential in the macOS Keychain via `security` CLI.
pub fn store(key: &str, value: &str) -> Result<(), String> {
    // Delete existing entry first (security add-generic-password fails if it exists)
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", key])
        .output();

    let output = std::process::Command::new("security")
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

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("failed to store credential '{key}': {stderr}"))
    }
}

/// Delete a credential from the macOS Keychain via `security` CLI.
pub fn delete(key: &str) -> Result<(), String> {
    let output = std::process::Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", key])
        .output()
        .map_err(|e| format!("failed to run security CLI: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("failed to delete credential '{key}': {stderr}"))
    }
}

/// Get a credential from the macOS Keychain via `security` CLI.
///
/// Uses `security find-generic-password -s scitadel -a <key> -w` which
/// reads from the login keychain without triggering per-binary auth prompts.
pub fn get_keychain(key: &str) -> Option<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", SERVICE, "-a", key, "-w"])
        .output()
        .ok()?;

    if output.status.success() {
        let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if val.is_empty() { None } else { Some(val) }
    } else {
        None
    }
}

/// Definitions of credentials required by each source.
pub struct SourceCredentials {
    pub source: &'static str,
    pub keys: &'static [CredentialKey],
}

pub struct CredentialKey {
    pub keychain_key: &'static str,
    pub env_var: &'static str,
    pub label: &'static str,
    pub secret: bool,
}

pub static PATENTSVIEW_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "patentsview",
    keys: &[CredentialKey {
        keychain_key: "patentsview.api_key",
        env_var: "SCITADEL_PATENTSVIEW_KEY",
        label: "API key",
        secret: true,
    }],
};

pub static PUBMED_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "pubmed",
    keys: &[CredentialKey {
        keychain_key: "pubmed.api_key",
        env_var: "SCITADEL_PUBMED_API_KEY",
        label: "API key",
        secret: true,
    }],
};

pub static OPENALEX_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "openalex",
    keys: &[CredentialKey {
        keychain_key: "openalex.email",
        env_var: "SCITADEL_OPENALEX_EMAIL",
        label: "Email (for polite pool)",
        secret: false,
    }],
};

pub static LENS_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "lens",
    keys: &[CredentialKey {
        keychain_key: "lens.api_token",
        env_var: "SCITADEL_LENS_TOKEN",
        label: "API token",
        secret: true,
    }],
};

pub static EPO_CREDENTIALS: SourceCredentials = SourceCredentials {
    source: "epo",
    keys: &[
        CredentialKey {
            keychain_key: "epo.consumer_key",
            env_var: "SCITADEL_EPO_KEY",
            label: "Consumer key",
            secret: false,
        },
        CredentialKey {
            keychain_key: "epo.consumer_secret",
            env_var: "SCITADEL_EPO_SECRET",
            label: "Consumer secret",
            secret: true,
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

/// Check whether a source has all required credentials configured.
pub fn check_source(creds: &SourceCredentials) -> Result<(), MissingCredential> {
    let missing: Vec<&CredentialKey> = creds
        .keys
        .iter()
        .filter(|k| resolve(k.keychain_key, k.env_var, "").is_none())
        .collect();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(MissingCredential {
            source: creds.source.to_string(),
            keys: missing.iter().map(|k| k.keychain_key.to_string()).collect(),
            env_vars: missing.iter().map(|k| k.env_var.to_string()).collect(),
            remedy: format!("scitadel auth login {}", creds.source),
        })
    }
}
