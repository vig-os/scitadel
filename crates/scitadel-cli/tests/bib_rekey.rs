#![allow(deprecated)] // `Command::cargo_bin` is the stable assert_cmd entry.

//! End-to-end `scitadel bib rekey` integration. Imports a small
//! fixture, then exercises the rekey CLI: explicit key, collision
//! rejection, algorithmic re-run, and the alias-preservation
//! breadcrumb that lets old citations keep resolving.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("tests");
    p.push("fixtures");
    p.push("zotero-export.bib");
    p
}

fn cmd(db: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("scitadel").unwrap();
    c.env("SCITADEL_DB", db);
    c
}

fn init_with_fixture(tmp: &TempDir) -> PathBuf {
    let db_path = tmp.path().join("scitadel.db");
    cmd(&db_path)
        .args([
            "init",
            "--yes",
            "--email",
            "test@example.com",
            "--sources",
            "openalex,arxiv",
            "--db",
        ])
        .arg(&db_path)
        .assert()
        .success();
    cmd(&db_path)
        .args(["bib", "import"])
        .arg(fixture_path())
        .args(["--reader", "test-reader"])
        .assert()
        .success();
    db_path
}

/// Pull any paper id from the seeded DB by parsing the bib-import
/// stdout. The `created` column has the first 8 chars of the id.
fn first_paper_id(db: &std::path::Path) -> String {
    let out = cmd(db)
        .args(["bib", "import"])
        .arg(fixture_path())
        .args(["--reader", "scrape"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);
    // Lines look like `  unchanged dead-beef  citekey ...` — grab the
    // 8-char id off the first per-paper line.
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("unchanged ") || trimmed.starts_with("created ") {
            // Skip past status word and surrounding whitespace.
            let after = trimmed.split_whitespace().nth(1).unwrap_or("");
            if !after.is_empty() {
                return after.to_string();
            }
        }
    }
    panic!("no paper id found in import output:\n{stdout}");
}

#[test]
fn rekey_with_explicit_key_succeeds_and_preserves_old_via_alias() {
    let tmp = TempDir::new().unwrap();
    let db = init_with_fixture(&tmp);
    let id_prefix = first_paper_id(&db);

    cmd(&db)
        .args(["bib", "rekey"])
        .arg(&id_prefix)
        .args(["--key", "explicit-target"])
        .args(["--reader", "lars"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rekeyed"))
        .stdout(predicate::str::contains("explicit-target"))
        .stdout(predicate::str::contains("preserved as alias"));
}

#[test]
fn rekey_collision_with_existing_paper_key_fails_loudly() {
    let tmp = TempDir::new().unwrap();
    let db = init_with_fixture(&tmp);
    let id_prefix = first_paper_id(&db);

    // First, rekey one paper to a known target.
    cmd(&db)
        .args(["bib", "rekey"])
        .arg(&id_prefix)
        .args(["--key", "taken-citekey"])
        .args(["--reader", "lars"])
        .assert()
        .success();

    // Now try to take that same key on a *different* paper. Need a
    // second prefix — pull a different one. Use the bib-import output
    // line for any other paper.
    let second_prefix = {
        let out = cmd(&db)
            .args(["bib", "import"])
            .arg(fixture_path())
            .args(["--reader", "scrape"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8_lossy(&out);
        let mut prefixes = stdout
            .lines()
            .filter_map(|l| {
                let t = l.trim_start();
                if t.starts_with("unchanged ") {
                    t.split_whitespace().nth(1).map(str::to_string)
                } else {
                    None
                }
            })
            .filter(|p| p != &id_prefix);
        prefixes.next().expect("second paper id available")
    };

    cmd(&db)
        .args(["bib", "rekey"])
        .arg(&second_prefix)
        .args(["--key", "taken-citekey"])
        .args(["--reader", "lars"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already used"));
}

#[test]
fn rekey_unknown_paper_yields_clear_error() {
    let tmp = TempDir::new().unwrap();
    let db = init_with_fixture(&tmp);

    cmd(&db)
        .args([
            "bib",
            "rekey",
            "deadbeef-no-such-paper",
            "--key",
            "anything",
            "--reader",
            "lars",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no paper matches id prefix"));
}

#[test]
fn rekey_invalid_key_rejected() {
    let tmp = TempDir::new().unwrap();
    let db = init_with_fixture(&tmp);
    let id_prefix = first_paper_id(&db);

    cmd(&db)
        .args(["bib", "rekey"])
        .arg(&id_prefix)
        .args(["--key", "1leadingdigit"])
        .args(["--reader", "lars"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid citation key"));
}

#[test]
fn rekey_algorithmic_with_unchanged_metadata_is_noop() {
    let tmp = TempDir::new().unwrap();
    let db = init_with_fixture(&tmp);
    let id_prefix = first_paper_id(&db);

    cmd(&db)
        .args(["bib", "rekey"])
        .arg(&id_prefix)
        .args(["--reader", "lars"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no-op"));
}
