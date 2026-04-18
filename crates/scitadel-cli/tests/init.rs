#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry for stable assert_cmd.

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// Non-interactive init writes a config and creates the DB at the given path.
#[test]
fn init_yes_writes_config_and_db() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("scitadel.db");
    let config_path = tmp.path().join("config.toml");

    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("init")
        .arg("--yes")
        .arg("--email")
        .arg("foo@bar.example")
        .arg("--sources")
        .arg("openalex,arxiv")
        .arg("--db")
        .arg(&db_path)
        // Suppress workspace-level config resolution so we're testing the fresh path.
        .env_remove("SCITADEL_DB")
        .assert()
        .success()
        .stdout(predicate::str::contains("Config written"))
        .stdout(predicate::str::contains("openalex, arxiv"));

    assert!(db_path.exists(), "DB should exist at {}", db_path.display());
    assert!(
        config_path.exists(),
        "config should be written to {}",
        config_path.display()
    );

    let written = fs::read_to_string(&config_path).unwrap();
    assert!(
        written.contains("default_sources"),
        "config missing default_sources:\n{written}"
    );
    assert!(
        written.contains("foo@bar.example"),
        "config missing email:\n{written}"
    );
}

/// Re-running init does not wipe existing values — idempotent.
#[test]
fn init_yes_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("scitadel.db");
    let config_path = tmp.path().join("config.toml");

    let cmd = || {
        Command::cargo_bin("scitadel")
            .unwrap()
            .arg("init")
            .arg("--yes")
            .arg("--email")
            .arg("a@b.example")
            .arg("--sources")
            .arg("openalex")
            .arg("--db")
            .arg(&db_path)
            .env_remove("SCITADEL_DB")
            .assert()
            .success();
    };

    cmd();
    let first = fs::read_to_string(&config_path).unwrap();
    cmd();
    let second = fs::read_to_string(&config_path).unwrap();
    assert_eq!(first, second, "re-running init changed the config");
}
