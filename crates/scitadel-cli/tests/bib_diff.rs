#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry for stable assert_cmd.

//! End-to-end coverage for `scitadel bib diff` (#135 sub-feature C).
//!
//! Exercises the file-vs-file path (BibTeX, CSL-JSON, mixed), the
//! `--question-id` path against a seeded DB, exit-code semantics, and
//! the `--no-color` toggle. Pure-logic / format-detection tests live in
//! `scitadel-export::diff::tests` and `scitadel-export::diff_input::tests`.

use std::path::Path;

use assert_cmd::Command;
use scitadel_core::models::{Paper, PaperId, ResearchQuestion};
use scitadel_core::ports::{PaperRepository, QuestionRepository};
use scitadel_db::sqlite::{Database, SqliteShortlistRepository};
use tempfile::TempDir;

const READER: &str = "test-reader";

fn write(p: &Path, s: &str) {
    std::fs::write(p, s).unwrap();
}

const BIB_A: &str = r"
@article{smith2024quantum,
    title = {Quantum Advantage},
    author = {Smith, John},
    year = {2024},
    doi = {10.1038/abc.2024}
}
@article{jones2022old,
    title = {Old Z},
    author = {Jones, Robert},
    year = {2022}
}
";

// Same as A but: jones2022old removed, lee2023neural added,
// smith2024quantum's title edited.
const BIB_B: &str = r"
@article{smith2024quantum,
    title = {Quantum Advantage Revisited},
    author = {Smith, John},
    year = {2024},
    doi = {10.1038/abc.2024}
}
@article{lee2023neural,
    title = {Neural Y},
    author = {Lee, Kai},
    year = {2023}
}
";

fn diff(file_a: &Path, file_b: &Path, extra_args: &[&str]) -> assert_cmd::assert::Assert {
    let mut cmd = Command::cargo_bin("scitadel").unwrap();
    cmd.arg("bib").arg("diff").arg(file_a).arg(file_b);
    for a in extra_args {
        cmd.arg(a);
    }
    cmd.assert()
}

/// Identical files exit 0 and the report says so.
#[test]
fn identical_files_exit_zero() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.bib");
    let b = tmp.path().join("b.bib");
    write(&a, BIB_A);
    write(&b, BIB_A);
    let assert = diff(&a, &b, &["--no-color"]).code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(out.contains("No differences"), "stdout: {out}");
}

/// Different files exit 1 with added/removed/changed sections.
#[test]
fn drifted_files_exit_one_with_text_report() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.bib");
    let b = tmp.path().join("b.bib");
    write(&a, BIB_A);
    write(&b, BIB_B);

    let assert = diff(&a, &b, &["--no-color"]).code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(out.contains("ADDED (1):"), "missing ADDED: {out}");
    assert!(out.contains("lee2023neural"), "lee not in added: {out}");
    assert!(out.contains("REMOVED (1):"), "missing REMOVED: {out}");
    assert!(out.contains("jones2022old"), "jones not in removed: {out}");
    assert!(out.contains("CHANGED (1):"), "missing CHANGED: {out}");
    assert!(out.contains("smith2024quantum"));
    assert!(
        out.contains("Quantum Advantage → Quantum Advantage Revisited"),
        "field change missing: {out}"
    );
}

/// `--no-color` (and / or non-TTY pipe) ⇒ no ANSI escapes in output.
#[test]
fn no_color_strips_ansi_escapes() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.bib");
    let b = tmp.path().join("b.bib");
    write(&a, BIB_A);
    write(&b, BIB_B);
    let assert = diff(&a, &b, &["--no-color"]).code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(!out.contains('\x1b'), "ANSI must not appear: {out:?}");
}

/// `--format json` emits a parsable BibDiff JSON.
#[test]
fn json_format_emits_structured_output() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.bib");
    let b = tmp.path().join("b.bib");
    write(&a, BIB_A);
    write(&b, BIB_B);
    let assert = diff(&a, &b, &["--format", "json"]).code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["added"].as_array().unwrap().len(), 1);
    assert_eq!(v["removed"].as_array().unwrap().len(), 1);
    assert_eq!(v["changed"].as_array().unwrap().len(), 1);
    assert_eq!(v["added"][0]["citekey"], "lee2023neural");
    assert_eq!(v["removed"][0]["citekey"], "jones2022old");
    assert_eq!(v["changed"][0]["citekey"], "smith2024quantum");
}

/// BibTeX-vs-CSL-JSON of the same shortlist produces zero diff.
#[test]
fn mixed_format_same_papers_zero_diff() {
    let tmp = TempDir::new().unwrap();
    let bib = tmp.path().join("paper.bib");
    let json = tmp.path().join("paper.json");
    write(
        &bib,
        r"
@article{smith2024,
    title = {Quantum},
    author = {Smith, John},
    year = {2024}
}
",
    );
    write(
        &json,
        r#"[
            {
                "id": "smith2024",
                "type": "article-journal",
                "title": "Quantum",
                "author": [{"family": "Smith", "given": "John"}],
                "issued": {"date-parts": [[2024]]}
            }
        ]"#,
    );
    diff(&bib, &json, &["--no-color"]).code(0);
}

// ---------- --question-id form ----------

fn seed_db(tmp: &Path) -> (std::path::PathBuf, String) {
    let db_path = tmp.join("scitadel.db");
    let db = Database::open(&db_path).unwrap();
    db.migrate().unwrap();

    let (paper_repo, _, q_repo, _, _) = db.repositories();
    let mut q = ResearchQuestion::new("Diff Q?");
    q.description = "test".into();
    q_repo.save_question(&q).unwrap();
    let qid = q.id.as_str().to_string();

    let papers = [
        ("p-aaa", "Attention Is All You Need", "Vaswani, A.", 2017),
        ("p-bbb", "Deep Residual Learning", "He, Kaiming", 2015),
    ];
    for (id, title, author, year) in papers {
        let mut p = Paper::new(title);
        p.id = PaperId::from(id);
        p.authors = vec![author.to_string()];
        p.year = Some(year);
        paper_repo.save(&p).unwrap();
        let shortlist = SqliteShortlistRepository::new(db.clone());
        shortlist.toggle(&qid, id, READER).unwrap();
    }
    (db_path, qid)
}

#[test]
fn question_id_form_compares_file_against_db_snapshot() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid) = seed_db(tmp.path());
    let bib_path = tmp.path().join("paper.bib");

    // First snapshot the DB to a file so we have a known-equal baseline.
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("snapshot")
        .arg(&qid)
        .arg("--output")
        .arg(&bib_path)
        .arg("--reader")
        .arg(READER)
        .env("SCITADEL_DB", &db_path)
        .assert()
        .success();

    // Now diff the file vs DB ⇒ exit 0.
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("diff")
        .arg(&bib_path)
        .arg("--question-id")
        .arg(&qid)
        .arg("--reader")
        .arg(READER)
        .arg("--no-color")
        .env("SCITADEL_DB", &db_path)
        .assert()
        .code(0);

    // Edit the file ⇒ exit 1.
    let mut content = std::fs::read_to_string(&bib_path).unwrap();
    content = content.replace("Vaswani, A.", "Vaswani, Ashish");
    std::fs::write(&bib_path, content).unwrap();
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("diff")
        .arg(&bib_path)
        .arg("--question-id")
        .arg(&qid)
        .arg("--reader")
        .arg(READER)
        .arg("--no-color")
        .env("SCITADEL_DB", &db_path)
        .assert()
        .code(1);
}

#[test]
fn missing_second_side_is_an_error() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.bib");
    write(&a, BIB_A);
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("diff")
        .arg(&a)
        .arg("--no-color")
        .assert()
        .failure();
}
