#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry for stable assert_cmd.

//! `scitadel auth` against the non-macOS credential backends (#212).
//!
//! Driving the real binary is the only way to test backend selection:
//! the backend is resolved once per process from the environment, so the
//! choice can't be flipped inside a single test binary.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// A `scitadel` invocation pinned to the file backend inside `tmp`, with
/// every credential env var cleared so the host's real setup can't leak in.
fn scitadel(store: &Path) -> Command {
    let mut cmd = Command::cargo_bin("scitadel").unwrap();
    cmd.env("SCITADEL_CREDENTIAL_BACKEND", "file")
        .env("SCITADEL_CREDENTIALS_FILE", store)
        .env_remove("SCITADEL_OPENALEX_API_KEY")
        .env_remove("SCITADEL_OPENALEX_EMAIL")
        .env_remove("SCITADEL_PUBMED_API_KEY")
        .env_remove("SCITADEL_PATENTSVIEW_KEY")
        .env_remove("SCITADEL_LENS_TOKEN")
        .env_remove("SCITADEL_EPO_KEY")
        .env_remove("SCITADEL_EPO_SECRET");
    cmd
}

fn store_path(tmp: &TempDir) -> PathBuf {
    tmp.path().join("scitadel").join("credentials.toml")
}

#[test]
fn auth_status_names_the_backend_in_use() {
    let tmp = TempDir::new().unwrap();
    scitadel(&store_path(&tmp))
        .arg("auth")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Credential store: file"))
        .stdout(predicate::str::contains("mode 0600"))
        .stdout(predicate::str::contains("SCITADEL_CREDENTIAL_BACKEND"));
}

#[test]
fn auth_status_reports_the_backend_for_a_stored_key_and_env_for_an_env_var() {
    let tmp = TempDir::new().unwrap();
    let store = store_path(&tmp);

    scitadel(&store)
        .arg("auth")
        .arg("login")
        .arg("openalex")
        .write_stdin("oa-key-123\nme@example.org\n")
        .assert()
        .success();

    scitadel(&store)
        .arg("auth")
        .arg("status")
        .env("SCITADEL_PUBMED_API_KEY", "pm-from-env")
        .assert()
        .success()
        // Stored in the file backend…
        .stdout(predicate::str::contains("API key: file"))
        // …resolved from the environment…
        .stdout(predicate::str::contains("API key: env"))
        // …and absent entirely.
        .stdout(predicate::str::contains("Consumer key: missing"))
        // Never the values themselves.
        .stdout(predicate::str::contains("oa-key-123").not())
        .stdout(predicate::str::contains("pm-from-env").not());
}

#[test]
fn auth_login_openalex_asks_for_both_the_key_and_the_email() {
    let tmp = TempDir::new().unwrap();
    let store = store_path(&tmp);

    scitadel(&store)
        .arg("auth")
        .arg("login")
        .arg("openalex")
        .write_stdin("oa-key-123\nme@example.org\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("API key"))
        .stdout(predicate::str::contains("Email (polite pool"))
        .stdout(predicate::str::contains("Stored: openalex.api_key"))
        .stdout(predicate::str::contains("Stored: openalex.email"))
        // The key must never be echoed back.
        .stdout(predicate::str::contains("oa-key-123").not());

    let written = fs::read_to_string(&store).unwrap();
    assert!(
        written.contains("oa-key-123") && written.contains("me@example.org"),
        "both credentials should be in the store:\n{written}"
    );

    scitadel(&store)
        .arg("auth")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("[+] openalex       configured"));
}

#[test]
fn the_polite_pool_email_is_optional() {
    let tmp = TempDir::new().unwrap();
    let store = store_path(&tmp);

    // Key, then a bare newline to skip the email.
    scitadel(&store)
        .arg("auth")
        .arg("login")
        .arg("openalex")
        .write_stdin("oa-key-123\n\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Stored: openalex.api_key"))
        .stdout(predicate::str::contains("Skipped: openalex.email"));

    // …and the source still counts as configured without it.
    scitadel(&store)
        .arg("auth")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("[+] openalex       configured"))
        .stdout(predicate::str::contains(
            "Email (polite pool, optional): missing",
        ));
}

#[test]
fn a_blank_required_credential_is_rejected() {
    let tmp = TempDir::new().unwrap();
    scitadel(&store_path(&tmp))
        .arg("auth")
        .arg("login")
        .arg("openalex")
        .write_stdin("\n\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("API key is required"));
}

#[test]
fn auth_logout_clears_the_stored_credentials() {
    let tmp = TempDir::new().unwrap();
    let store = store_path(&tmp);

    scitadel(&store)
        .arg("auth")
        .arg("login")
        .arg("openalex")
        .write_stdin("oa-key-123\nme@example.org\n")
        .assert()
        .success();

    scitadel(&store)
        .arg("auth")
        .arg("logout")
        .arg("openalex")
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed: openalex.api_key"));

    let written = fs::read_to_string(&store).unwrap();
    assert!(
        !written.contains("oa-key-123"),
        "logout must remove the secret:\n{written}"
    );

    scitadel(&store)
        .arg("auth")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "[-] openalex       not configured",
        ));
}

#[cfg(unix)]
#[test]
fn the_file_backend_store_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let store = store_path(&tmp);

    scitadel(&store)
        .arg("auth")
        .arg("login")
        .arg("lens")
        .write_stdin("lens-token\n")
        .assert()
        .success();

    let mode = fs::metadata(&store).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "credential file must be owner-read/write only");
}

/// On a box with no Secret Service and no macOS `security`, detection must
/// land on the file backend by itself — no env override, no crash. This is
/// the exact situation from #212, where `auth login` died with
/// "failed to run security CLI: No such file or directory".
#[cfg(target_os = "linux")]
#[test]
fn auth_login_works_on_a_linux_box_with_no_secret_service() {
    let tmp = TempDir::new().unwrap();
    let store = store_path(&tmp);

    let mut cmd = Command::cargo_bin("scitadel").unwrap();
    cmd.env("SCITADEL_CREDENTIALS_FILE", &store)
        .env_remove("SCITADEL_CREDENTIAL_BACKEND")
        // An empty PATH means neither `security` nor `secret-tool` is found,
        // so detection has to fall through to the file store.
        .env("PATH", "")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("SCITADEL_PUBMED_API_KEY")
        .arg("auth")
        .arg("login")
        .arg("pubmed")
        .write_stdin("pm-key\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Stored: pubmed.api_key"));

    assert!(fs::read_to_string(&store).unwrap().contains("pm-key"));
}

/// A `secret-tool` stand-in, so the Secret Service path is covered on
/// machines (CI included) with no keyring daemon. It implements the exact
/// contract the backend relies on: `store` reads the secret from stdin,
/// `lookup` prints it with no trailing newline and exits non-zero when the
/// item is absent, `clear` removes it — and, critically, a *reachable*
/// service keeps stderr silent, which is how detection tells it apart from
/// a missing D-Bus session.
///
/// Not on macOS: detection prefers the Keychain there, so the stub is
/// never consulted.
#[cfg(all(unix, not(target_os = "macos")))]
const SECRET_TOOL_STUB: &str = r#"#!/bin/sh
set -eu
db="$SECRET_TOOL_STUB_DB"
touch "$db"
# argv: <verb> [--label X] service <svc> account <key>
verb="$1"; shift
[ "$verb" = "store" ] && { shift; shift; }   # drop --label <value>
key="$4"                                      # service <svc> account <key>
case "$verb" in
  store)  secret="$(cat)"
          grep -v "^$key	" "$db" > "$db.new" 2>/dev/null || true
          printf '%s\t%s\n' "$key" "$secret" >> "$db.new"
          mv "$db.new" "$db" ;;
  lookup) line="$(grep "^$key	" "$db" || true)"
          [ -n "$line" ] || exit 1
          printf '%s' "${line#*	}" ;;
  clear)  grep -q "^$key	" "$db" || exit 1
          grep -v "^$key	" "$db" > "$db.new" || true
          mv "$db.new" "$db" ;;
  *)      echo "unsupported verb $verb" >&2; exit 2 ;;
esac
"#;

/// `dir` first on `$PATH`, so the stub shadows any real `secret-tool`
/// while the shell utilities it calls stay reachable.
#[cfg(all(unix, not(target_os = "macos")))]
fn path_with(dir: &Path) -> std::ffi::OsString {
    let mut entries = vec![dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(entries).unwrap()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn install_secret_tool_stub(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = dir.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let stub = bin_dir.join("secret-tool");
    fs::write(&stub, SECRET_TOOL_STUB).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

/// With a Secret Service reachable, detection must pick it over the file
/// store — and round-trip credentials through it.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn secret_service_is_preferred_when_secret_tool_answers() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = install_secret_tool_stub(tmp.path());
    let keyring = tmp.path().join("keyring.tsv");
    let unused_file_store = tmp.path().join("should-not-be-written.toml");

    let scitadel = || {
        let mut cmd = Command::cargo_bin("scitadel").unwrap();
        cmd.env("PATH", path_with(&bin_dir))
            .env("SECRET_TOOL_STUB_DB", &keyring)
            .env("SCITADEL_CREDENTIALS_FILE", &unused_file_store)
            .env_remove("SCITADEL_CREDENTIAL_BACKEND")
            .env_remove("SCITADEL_OPENALEX_API_KEY")
            .env_remove("SCITADEL_OPENALEX_EMAIL");
        cmd
    };

    scitadel()
        .arg("auth")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Credential store: secret-service"));

    scitadel()
        .arg("auth")
        .arg("login")
        .arg("openalex")
        .write_stdin("oa-key-123\nme@example.org\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Stored: openalex.api_key"))
        .stdout(predicate::str::contains("oa-key-123").not());

    // The secret went to the keyring, via stdin — never onto the argv, and
    // never into the file store.
    let keyring_contents = fs::read_to_string(&keyring).unwrap();
    assert!(
        keyring_contents.contains("openalex.api_key\toa-key-123"),
        "secret-tool stub should hold the key:\n{keyring_contents}"
    );
    assert!(
        !unused_file_store.exists(),
        "the file fallback must not be touched when a Secret Service exists"
    );

    scitadel()
        .arg("auth")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("API key: secret-service"))
        .stdout(predicate::str::contains("[+] openalex       configured"));

    scitadel()
        .arg("auth")
        .arg("logout")
        .arg("openalex")
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed: openalex.api_key"));
    assert!(!fs::read_to_string(&keyring).unwrap().contains("oa-key-123"));
}

/// `secret-tool` on PATH but no D-Bus session: the probe writes to stderr,
/// and detection must fall back to the file store instead of failing every
/// `auth login` — the failure mode reported in #212.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn an_unreachable_secret_service_falls_back_to_the_file_store() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let stub = bin_dir.join("secret-tool");
    fs::write(
        &stub,
        "#!/bin/sh\necho 'Cannot autolaunch D-Bus without X11 $DISPLAY' >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    let store = tmp.path().join("credentials.toml");
    Command::cargo_bin("scitadel")
        .unwrap()
        .env("PATH", path_with(&bin_dir))
        .env("SCITADEL_CREDENTIALS_FILE", &store)
        .env_remove("SCITADEL_CREDENTIAL_BACKEND")
        .env_remove("SCITADEL_PUBMED_API_KEY")
        .arg("auth")
        .arg("login")
        .arg("pubmed")
        .write_stdin("pm-key\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Stored: pubmed.api_key"));

    assert!(fs::read_to_string(&store).unwrap().contains("pm-key"));
}
