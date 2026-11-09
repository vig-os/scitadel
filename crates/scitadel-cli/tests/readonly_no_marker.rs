//! Round-5 adversarial regression (#266 break #2): a read-only
//! CLI command in a fresh directory used to call `load_config()` →
//! `Database::open()` → `create_dir_all(.scitadel/)`, seeding a
//! marker under whatever dir the user happened to be in. A later
//! `scitadel mcp` launched from a parent of that dir then accepted
//! the fresh marker as a wide confinement root. Read-only commands
//! must now refuse with a clear message and leave no `.scitadel/`
//! behind.

#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry on stable assert_cmd.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// `scitadel history` in a fresh directory must not create a
/// `.scitadel/` and must exit non-zero with "no scitadel project".
#[test]
fn history_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    let tmp = TempDir::new().unwrap();
    // Belt-and-braces: strip any `SCITADEL_WORKSPACE` / `SCITADEL_DB`
    // that could leak in from the parent env and defeat the test.
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("history")
        .current_dir(tmp.path())
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .assert()
        .failure()
        .stderr(predicate::str::contains("scitadel init"));

    assert!(
        !tmp.path().join(".scitadel").exists(),
        "a read-only command must not seed a .scitadel/ dir"
    );
}

/// Same guarantee for `scitadel coverage`, which was the other
/// adversarial example (any command that opens the DB via
/// `read_action_list`).
#[test]
fn coverage_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("coverage")
        .current_dir(tmp.path())
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .assert()
        .failure()
        .stderr(predicate::str::contains("scitadel init"));
    assert!(
        !tmp.path().join(".scitadel").exists(),
        "a read-only command must not seed a .scitadel/ dir"
    );
}
