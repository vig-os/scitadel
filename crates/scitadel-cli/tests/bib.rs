#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry for stable assert_cmd.

//! End-to-end coverage for `scitadel bib snapshot` / `scitadel bib verify` (#178).
//!
//! Tests seed an in-process scitadel-db file directly (there's no CLI
//! command for shortlist toggling yet — the TUI/MCP own that surface),
//! then shell out the CLI binary with `SCITADEL_DB` pointing at the
//! seeded file. This mirrors the workflow CI users actually exercise.

use std::path::Path;

use assert_cmd::Command;
use scitadel_core::models::{Paper, PaperId, ResearchQuestion};
use scitadel_core::ports::{PaperRepository, QuestionRepository};
use scitadel_db::sqlite::{Database, SqliteShortlistRepository};
use tempfile::TempDir;

const READER: &str = "test-reader";

/// Seed a fresh DB with one question + N papers shortlisted under
/// `READER`. Returns `(db_path, question_id, paper_ids)`.
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

fn snapshot(db_path: &Path, question_id: &str, output: &Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("snapshot")
        .arg(question_id)
        .arg("--output")
        .arg(output)
        .arg("--reader")
        .arg(READER)
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

fn sidecar_for(bib: &Path) -> std::path::PathBuf {
    let mut s = bib.as_os_str().to_owned();
    s.push(".scitadel-bib.lock");
    std::path::PathBuf::from(s)
}

/// Snapshot must be byte-identical (modulo `generated_at`) on the
/// second run — that's the whole determinism story per #178.
#[test]
fn snapshot_is_byte_deterministic_across_runs() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let bib = tmp.path().join("paper.bib");

    snapshot(&db_path, &qid, &bib).success();
    let bib_first = std::fs::read_to_string(&bib).unwrap();
    let lock_first: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sidecar_for(&bib)).unwrap()).unwrap();

    // Sleep is tempting here but unnecessary: only `generated_at`
    // differs across runs, and we explicitly exclude it from the
    // determinism comparison below.
    snapshot(&db_path, &qid, &bib).success();
    let bib_second = std::fs::read_to_string(&bib).unwrap();
    let lock_second: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sidecar_for(&bib)).unwrap()).unwrap();

    assert_eq!(
        bib_first, bib_second,
        ".bib must be byte-identical across runs"
    );
    assert_eq!(
        lock_first["content_hash"], lock_second["content_hash"],
        "content_hash must match across runs"
    );
    assert_eq!(
        lock_first["shortlist_hash"], lock_second["shortlist_hash"],
        "shortlist_hash must match across runs"
    );
    assert_eq!(lock_first["algo_hash"], lock_second["algo_hash"]);
    assert_eq!(
        lock_first["scitadel_version"],
        lock_second["scitadel_version"]
    );
    // generated_at is allowed to differ — that's the whole point.
}

/// Fresh-snapshot-then-verify must exit 0.
#[test]
fn verify_returns_zero_on_fresh_snapshot() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let bib = tmp.path().join("paper.bib");
    snapshot(&db_path, &qid, &bib).success();
    verify(&db_path, &bib).code(0);
}

/// Modify the `.bib` after snapshot ⇒ exit 1 (drift) with diff text.
#[test]
fn verify_returns_one_on_content_drift() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let bib = tmp.path().join("paper.bib");
    snapshot(&db_path, &qid, &bib).success();

    let mut content = std::fs::read_to_string(&bib).unwrap();
    content.push_str("\n@misc{drift,\n  title = {oops},\n}\n");
    std::fs::write(&bib, content).unwrap();

    let assert = verify(&db_path, &bib).code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(out.contains("DRIFT"), "stderr: {out}");
    assert!(
        out.contains("--- committed") || out.contains("+++ regenerated"),
        "expected diff text, got: {out}"
    );
}

/// Manually flip `algo_hash` in the sidecar ⇒ exit 2 (stale).
#[test]
fn verify_returns_two_on_algo_hash_stale() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let bib = tmp.path().join("paper.bib");
    snapshot(&db_path, &qid, &bib).success();

    let sidecar = sidecar_for(&bib);
    let mut lock: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    lock["algo_hash"] = serde_json::Value::String("deadbeef-stale".into());
    std::fs::write(&sidecar, serde_json::to_string_pretty(&lock).unwrap()).unwrap();

    let assert = verify(&db_path, &bib).code(2);
    let out = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(out.contains("STALE"), "stderr: {out}");
    assert!(out.contains("algo_hash"), "stderr: {out}");
}

/// Manually flip `scitadel_version` in the sidecar ⇒ exit 2 (stale).
#[test]
fn verify_returns_two_on_scitadel_version_stale() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let bib = tmp.path().join("paper.bib");
    snapshot(&db_path, &qid, &bib).success();

    let sidecar = sidecar_for(&bib);
    let mut lock: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    lock["scitadel_version"] = serde_json::Value::String("0.0.0-time-traveler".into());
    std::fs::write(&sidecar, serde_json::to_string_pretty(&lock).unwrap()).unwrap();

    let assert = verify(&db_path, &bib).code(2);
    let out = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(out.contains("STALE"), "stderr: {out}");
    assert!(out.contains("scitadel_version"), "stderr: {out}");
}

/// Sidecar absent ⇒ exit 2 with a useful "run snapshot" hint.
#[test]
fn verify_returns_two_when_sidecar_missing() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let bib = tmp.path().join("paper.bib");
    snapshot(&db_path, &qid, &bib).success();
    std::fs::remove_file(sidecar_for(&bib)).unwrap();

    let assert = verify(&db_path, &bib).code(2);
    let out = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        out.contains("STALE") && out.contains("no lockfile"),
        "stderr: {out}"
    );
}

/// `--no-lock` skips the sidecar; verify then reports stale.
#[test]
fn snapshot_no_lock_skips_sidecar() {
    let tmp = TempDir::new().unwrap();
    let (db_path, qid, _) = seed_db(tmp.path());
    let bib = tmp.path().join("paper.bib");

    Command::cargo_bin("scitadel")
        .unwrap()
        .arg("bib")
        .arg("snapshot")
        .arg(&qid)
        .arg("--output")
        .arg(&bib)
        .arg("--reader")
        .arg(READER)
        .arg("--no-lock")
        .env("SCITADEL_DB", &db_path)
        .assert()
        .success();

    assert!(bib.exists());
    assert!(!sidecar_for(&bib).exists(), "--no-lock should skip sidecar");
}
