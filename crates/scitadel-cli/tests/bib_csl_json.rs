#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry for stable assert_cmd.

//! End-to-end coverage for `scitadel bib snapshot --format csl-json` /
//! `scitadel bib verify` against a CSL-JSON sidecar (#135 sub-feature A).
//!
//! Mirrors the BibTeX-flavored cases in `bib.rs` so the JSON code path
//! exercises the same drift / stale / ok semantics the BibTeX path
//! already proves out.

use std::path::Path;

use assert_cmd::Command;
use scitadel_core::models::{Paper, PaperId, ResearchQuestion};
use scitadel_core::ports::{PaperRepository, QuestionRepository};
use scitadel_db::sqlite::{Database, SqliteShortlistRepository};
use tempfile::TempDir;

const READER: &str = "test-reader";

fn seed_db(tmp: &Path) -> (std::path::PathBuf, String, Vec<String>) {
    let db_path = tmp.join("scitadel.db");
    let db = Database::open(&db_path).unwrap();
    db.migrate().unwrap();

    let (paper_repo, _, q_repo, _, _) = db.repositories();
    let mut q = ResearchQuestion::new("Does X cause Y?");
    q.description = "test question".into();
    q_repo.save_question(&q).unwrap();
    let question_id = q.id.as_str().to_string();

    let papers = [
        ("p-aaa", "Attention Is All You Need", &["Vaswani, A."], 2017),
        ("p-bbb", "Deep Residual Learning", &["He, Kaiming"], 2015),
    ];
    let mut paper_ids = Vec::new();
    for (id, title, authors, year) in papers {
        let mut p = Paper::new(title);
        p.id = PaperId::from(id);
        p.authors = authors.iter().map(|s| (*s).to_string()).collect();
        p.year = Some(year);
        paper_repo.save(&p).unwrap();
        paper_ids.push(id.to_string());
    }

    let shortlist = SqliteShortlistRepository::new(db.clone());
    for pid in &paper_ids {
        shortlist.toggle(&question_id, pid, READER).unwrap();
    }

    (db_path, question_id, paper_ids)
}

fn snapshot_csl(db_path: &Path, question_id: &str, output: &Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("snapshot")
        .arg(question_id)
        .arg("--output")
        .arg(output)
        .arg("--reader")
        .arg(READER)
        .arg("--format")
        .arg("csl-json")
        .env("SCITADEL_DB", db_path)
        .assert()
}

fn verify(db_path: &Path, file: &Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("verify")
        .arg(file)
        .env("SCITADEL_DB", db_path)
        .assert()
}

fn sidecar_for(file: &Path) -> std::path::PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".scitadel-bib.lock");
    std::path::PathBuf::from(s)
}

/// CSL-JSON snapshot must be byte-identical across runs (sidecar
/// `generated_at` excepted) — same determinism contract as BibTeX.
#[test]
fn csl_snapshot_is_byte_deterministic_across_runs() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let out = tmp.path().join("paper.json");

    snapshot_csl(&db_path, &qid, &out).success();
    let first = std::fs::read_to_string(&out).unwrap();
    let lock_first: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sidecar_for(&out)).unwrap()).unwrap();

    snapshot_csl(&db_path, &qid, &out).success();
    let second = std::fs::read_to_string(&out).unwrap();
    let lock_second: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sidecar_for(&out)).unwrap()).unwrap();

    assert_eq!(first, second, ".json must be byte-identical across runs");
    assert_eq!(lock_first["content_hash"], lock_second["content_hash"]);
    assert_eq!(lock_first["shortlist_hash"], lock_second["shortlist_hash"]);
    assert_eq!(lock_first["format"], "csl-json");
}

/// Output is a valid JSON array with one entry per shortlisted paper,
/// authoritative on canonical CSL field names.
#[test]
fn csl_snapshot_emits_canonical_field_names() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let out = tmp.path().join("paper.json");
    snapshot_csl(&db_path, &qid, &out).success();

    let content = std::fs::read_to_string(&out).unwrap();
    let arr: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
    assert_eq!(arr.len(), 2);
    for entry in &arr {
        assert!(entry["id"].is_string());
        assert!(entry["type"].is_string());
        assert!(entry["title"].is_string());
        // Canonical schema: NO `journal`, `doi`, `url`, `keywords`.
        assert!(entry.get("journal").is_none());
        assert!(entry.get("doi").is_none());
        assert!(entry.get("url").is_none());
        assert!(entry.get("keywords").is_none());
    }
}

/// Fresh-snapshot-then-verify must exit 0 against a CSL-JSON sidecar.
#[test]
fn csl_verify_returns_zero_on_fresh_snapshot() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let out = tmp.path().join("paper.json");
    snapshot_csl(&db_path, &qid, &out).success();
    verify(&db_path, &out).code(0);
}

/// Modify the `.json` after snapshot ⇒ exit 1 (drift) — verify routes
/// on the sidecar's `format: "csl-json"` to compare against the
/// JSON-emitted regenerate, not BibTeX.
#[test]
fn csl_verify_returns_one_on_content_drift() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let out = tmp.path().join("paper.json");
    snapshot_csl(&db_path, &qid, &out).success();

    // Drop a key by string-replace to simulate human edit. Canonical
    // CSL has `"title"` → flip its value so verify sees a real diff.
    let original = std::fs::read_to_string(&out).unwrap();
    let modified = original.replace("\"Deep Residual", "\"Deep Residual Edited");
    assert_ne!(original, modified, "sanity: replacement must alter bytes");
    std::fs::write(&out, modified).unwrap();

    let assert = verify(&db_path, &out).code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(stderr.contains("DRIFT"), "stderr: {stderr}");
}

/// Default `--output` for `--format csl-json` is `paper.json`. Use
/// `cwd=tmp` so the file lands in the temp dir and we can check it.
#[test]
fn csl_snapshot_default_output_is_paper_json() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());

    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("snapshot")
        .arg(&qid)
        .arg("--reader")
        .arg(READER)
        .arg("--format")
        .arg("csl-json")
        .env("SCITADEL_DB", &db_path)
        .current_dir(tmp.path())
        .assert()
        .success();

    let default_out = tmp.path().join("paper.json");
    assert!(default_out.exists(), "default output must be paper.json");
    // sanity: it parses as JSON
    let content = std::fs::read_to_string(&default_out).unwrap();
    let _: serde_json::Value = serde_json::from_str(&content).unwrap();
}

/// Sidecar's `format` field records `"csl-json"`. Verify reads it.
#[test]
fn csl_sidecar_format_field_is_csl_json() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let out = tmp.path().join("paper.json");
    snapshot_csl(&db_path, &qid, &out).success();
    let lock: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sidecar_for(&out)).unwrap()).unwrap();
    assert_eq!(lock["format"], "csl-json");
    assert_eq!(lock["format_version"], "1");
}

/// BibTeX path stays 100% backwards compatible — same default output
/// (`paper.bib`), same sidecar `format: "bibtex"`.
#[test]
fn bibtex_path_remains_backwards_compatible() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());

    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("snapshot")
        .arg(&qid)
        .arg("--reader")
        .arg(READER)
        .env("SCITADEL_DB", &db_path)
        .current_dir(tmp.path())
        .assert()
        .success();

    let bib = tmp.path().join("paper.bib");
    assert!(bib.exists(), "default bibtex output must be paper.bib");
    let lock: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sidecar_for(&bib)).unwrap()).unwrap();
    assert_eq!(lock["format"], "bibtex");
}
