//! Round-5 adversarial regression (#266 break #2): a read-only
//! CLI command in a fresh directory used to call `load_config()` →
//! `Database::open()` → `create_dir_all(.scitadel/)`, seeding a
//! marker under whatever dir the user happened to be in. A later
//! `scitadel mcp` launched from a parent of that dir then accepted
//! the fresh marker as a wide confinement root. Read-only commands
//! must now refuse with a clear message and leave no `.scitadel/`
//! behind.
//!
//! Round-6 pins every project-aware command the same way (`bib
//! snapshot`, `bib rekey`, `bib diff --question-id`, `resolve-doi
//! --no-save`, `acquire --dry-run`, `gc`, `tui`) and adds the
//! `$HOME`-as-project regression: a user with a `.scitadel/` in
//! their home directory kept the full CLI working before round 5;
//! the round-5 gate reused the MCP confinement rules
//! (`reject_system_root`) and started refusing `history` /
//! `question list` there with "root is $HOME". Only MCP needs
//! that rule — the CLI's gate is a marker-existence probe.

#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry on stable assert_cmd.

use std::fs;

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

/// Round-6 additions: every project-aware command that used to
/// silently seed `.scitadel/` under cwd now refuses with the init
/// hint instead. One parameterised assertion covers all of them so
/// a new auto-initialising read-only command cannot forget the gate.
fn assert_refuses_without_marker(args: &[&str]) {
    let tmp = TempDir::new().unwrap();
    Command::cargo_bin("scitadel")
        .unwrap()
        .args(args)
        .current_dir(tmp.path())
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .assert()
        .failure()
        .stderr(predicate::str::contains("scitadel init"));
    assert!(
        !tmp.path().join(".scitadel").exists(),
        "{:?} must not seed a .scitadel/ dir",
        args
    );
}

#[test]
fn bib_snapshot_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    assert_refuses_without_marker(&["bib", "snapshot", "somequestion"]);
}

#[test]
fn bib_rekey_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    assert_refuses_without_marker(&["bib", "rekey", "somepaper"]);
}

#[test]
fn gc_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    assert_refuses_without_marker(&["gc"]);
}

#[test]
fn gc_dry_run_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    assert_refuses_without_marker(&["gc", "--dry-run"]);
}

#[test]
fn acquire_dry_run_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    assert_refuses_without_marker(&["acquire", "--dry-run"]);
}

#[test]
fn resolve_doi_no_save_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    assert_refuses_without_marker(&["resolve-doi", "10.1214/aos/1176344136", "--no-save"]);
}

#[test]
fn tui_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    assert_refuses_without_marker(&["tui"]);
}

/// `bib diff a.bib --question-id x` is the DB arm of diff, so it
/// refuses without a marker; the file-vs-file arm is unaffected.
#[test]
fn bib_diff_question_id_in_a_fresh_dir_refuses_and_leaves_no_marker() {
    let tmp = TempDir::new().unwrap();
    let bib = tmp.path().join("a.bib");
    fs::write(&bib, "@article{foo,title={x},author={y}}\n").unwrap();
    Command::cargo_bin("scitadel")
        .unwrap()
        .args(["bib", "diff"])
        .arg(&bib)
        .args(["--question-id", "someq"])
        .current_dir(tmp.path())
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .assert()
        .failure()
        .stderr(predicate::str::contains("scitadel init"));
    assert!(
        !tmp.path().join(".scitadel").exists(),
        "bib diff --question-id must not seed a .scitadel/ dir"
    );
}

/// Round-6 CLI regression: a user with a `.scitadel/` in `$HOME`
/// kept the full CLI working before round 5. The round-5 gate
/// shared the MCP `reject_system_root` rule and started refusing
/// `history` there with "root is $HOME". Only MCP needs that rule
/// — the CLI's gate is a marker-existence probe. We fake `$HOME`
/// via the env var (scitadel's project resolver consults it) and
/// put the marker under the fake home.
#[test]
fn history_in_a_home_project_succeeds() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    fs::create_dir_all(home.join(".scitadel")).unwrap();
    // `history` needs a usable DB — point SCITADEL_DB at a file in
    // the fake home so the command has something to read.
    let db_path = home.join(".scitadel").join("scitadel.db");
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("init")
        .arg("--yes")
        .arg("--email")
        .arg("foo@example.test")
        .arg("--db")
        .arg(&db_path)
        .env("HOME", home)
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .current_dir(home)
        .assert()
        .success();

    // Now `history` from `$HOME` must succeed. Before the round-6
    // fix this failed with "root is $HOME".
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("history")
        .env("HOME", home)
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .current_dir(home)
        .assert()
        .success();
}

/// Same CLI regression, exercised through `question list` (the
/// other command the round-6 brief called out by name).
#[test]
fn question_list_in_a_home_project_succeeds() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    fs::create_dir_all(home.join(".scitadel")).unwrap();
    let db_path = home.join(".scitadel").join("scitadel.db");
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("init")
        .arg("--yes")
        .arg("--email")
        .arg("foo@example.test")
        .arg("--db")
        .arg(&db_path)
        .env("HOME", home)
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .current_dir(home)
        .assert()
        .success();

    Command::cargo_bin("scitadel")
        .unwrap()
        .args(["question", "list"])
        .env("HOME", home)
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .current_dir(home)
        .assert()
        .success();
}
