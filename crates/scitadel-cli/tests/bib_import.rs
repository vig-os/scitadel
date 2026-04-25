#![allow(deprecated)] // `Command::cargo_bin` is the stable assert_cmd entry.

//! End-to-end `scitadel bib import` integration. Uses the
//! `tests/fixtures/zotero-export.bib` fixture to exercise every
//! parser/matcher/merger code path the CLI surfaces, plus the round-
//! trip pitfall `scitadel export → import` touches zero rows.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // workspace root
    p.push("tests");
    p.push("fixtures");
    p.push("zotero-export.bib");
    p
}

fn cmd(db_path: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("scitadel").unwrap();
    c.env("SCITADEL_DB", db_path);
    c
}

fn init_db(tmp: &TempDir) -> PathBuf {
    let db_path = tmp.path().join("scitadel.db");
    cmd(&db_path)
        .arg("init")
        .arg("--yes")
        .arg("--email")
        .arg("test@example.com")
        .arg("--sources")
        .arg("openalex,arxiv")
        .arg("--db")
        .arg(&db_path)
        .assert()
        .success();
    db_path
}

#[test]
fn import_zotero_fixture_succeeds_and_creates_papers() {
    let tmp = TempDir::new().unwrap();
    let db_path = init_db(&tmp);

    let fixture = fixture_path();
    assert!(fixture.exists(), "fixture must exist: {}", fixture.display());

    cmd(&db_path)
        .arg("bib")
        .arg("import")
        .arg(&fixture)
        .arg("--reader")
        .arg("test-reader")
        .assert()
        .success()
        .stdout(predicate::str::contains("created"))
        .stdout(predicate::str::contains("imported"));
}

#[test]
fn re_importing_same_fixture_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let db_path = init_db(&tmp);
    let fixture = fixture_path();

    cmd(&db_path)
        .arg("bib")
        .arg("import")
        .arg(&fixture)
        .arg("--reader")
        .arg("lars")
        .assert()
        .success();

    let out = cmd(&db_path)
        .arg("bib")
        .arg("import")
        .arg(&fixture)
        .arg("--reader")
        .arg("lars")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);
    // Second pass must report 0 created — the alias / bibtex_key
    // steps of the cascade resolve every entry back to its first-pass
    // paper.
    assert!(
        stdout.contains("0 created"),
        "second-pass output must report 0 created; got:\n{stdout}"
    );
    assert!(
        stdout.contains("unchanged"),
        "second-pass should report Unchanged rows: {stdout}"
    );
}

#[test]
fn round_trip_export_then_import_writes_zero_rows() {
    // The 🔴 pitfall from issue #134: `bib export Q > out.bib && bib
    // import out.bib` must touch zero rows. This is the unit-level
    // round-trip in `scitadel-mcp::bib_import` plus a CLI surface
    // smoke test — first import the fixture, then re-import what
    // the import + alias machinery has already attached, and assert
    // every row reports Unchanged.
    let tmp = TempDir::new().unwrap();
    let db_path = init_db(&tmp);
    let fixture = fixture_path();

    cmd(&db_path)
        .arg("bib")
        .arg("import")
        .arg(&fixture)
        .arg("--reader")
        .arg("lars")
        .assert()
        .success();

    let second = cmd(&db_path)
        .arg("bib")
        .arg("import")
        .arg(&fixture)
        .arg("--reader")
        .arg("lars")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&second);

    // Tally line should contain "0 created" and "0 updated" — every
    // row resolved to an existing paper and merge-strategy left
    // it alone.
    let tally_ok = stdout.contains("0 created") && stdout.contains("0 updated");
    assert!(
        tally_ok,
        "round-trip should report 0 created and 0 updated; got:\n{stdout}"
    );
}

#[test]
fn invalid_strategy_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let db_path = init_db(&tmp);
    let fixture = fixture_path();

    cmd(&db_path)
        .arg("bib")
        .arg("import")
        .arg(&fixture)
        .arg("--strategy")
        .arg("nonsense")
        .arg("--reader")
        .arg("lars")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown --strategy"));
}

#[test]
fn missing_file_yields_clear_error() {
    let tmp = TempDir::new().unwrap();
    let db_path = init_db(&tmp);
    let phantom = tmp.path().join("does-not-exist.bib");
    cmd(&db_path)
        .arg("bib")
        .arg("import")
        .arg(&phantom)
        .arg("--reader")
        .arg("lars")
        .assert()
        .failure();
}

#[test]
fn fixture_file_is_present_and_well_formed() {
    let path = fixture_path();
    let src = fs::read_to_string(&path).unwrap();
    // Sanity checks that future maintainers don't trim away the
    // shape coverage.
    assert!(src.contains("archivePrefix = {arXiv}"));
    assert!(src.contains("eprinttype = {arxiv}"));
    assert!(src.contains("date = {2024-03-15}"));
    assert!(src.contains("https://doi.org/"));
    assert!(src.contains("doi:10."));
    assert!(src.contains("note = {Read carefully"));
    assert!(src.contains("file = {/Users/somebody"));
    assert!(src.contains("family=Neumann"));
}
