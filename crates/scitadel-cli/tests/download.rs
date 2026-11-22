#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry for stable assert_cmd.

//! End-to-end `scitadel download` — the review-flagged shapes.
//!
//! The command itself touches OpenAlex and the publisher ladder, which no CLI
//! test can drive without a live network. These tests therefore focus on the
//! *boundary* that unit tests do not reach: the `--output-dir` deprecation
//! surface (#289 blocker 2). A malformed DOI short-circuits before any
//! network call, so the warning line and the exit path are observable
//! without a mock server.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// `--output-dir` is deprecated: the library location is managed by
/// `scitadel init`'s config since #289 (`download` writes into the
/// content-addressed blob store under the library root, not into the
/// user-chosen directory), so a caller that still passes the flag must see a
/// one-line warning on stderr naming the value they asked for and pointing at
/// the forward path.
///
/// The test drives a malformed DOI so the warning is emitted *and* the
/// command exits before touching the network. A valid DOI would need a mock
/// OpenAlex to be anything other than flaky here.
#[test]
fn download_output_dir_prints_deprecation_warning() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("my-papers");
    let db_path = tmp.path().join("scitadel.db");

    Command::cargo_bin("scitadel")
        .unwrap()
        .env("SCITADEL_DB", &db_path)
        .arg("download")
        .arg("not-a-doi") // fails `validate_doi_detailed` without any network
        .arg("--output-dir")
        .arg(&out)
        .assert()
        .failure()
        // The deprecation warning fires *before* the DOI validation does, so
        // it is on stderr even on the failure path.
        .stderr(predicate::str::contains("--output-dir is ignored"))
        .stderr(predicate::str::contains(out.display().to_string()))
        .stderr(predicate::str::contains("scitadel export"));
}

/// Without `--output-dir`, no deprecation warning appears. The flag's absence
/// is the common case now, and surfacing a warning here would be noise.
#[test]
fn download_without_output_dir_emits_no_deprecation_warning() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("scitadel.db");

    Command::cargo_bin("scitadel")
        .unwrap()
        .env("SCITADEL_DB", &db_path)
        .arg("download")
        .arg("not-a-doi")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--output-dir").not());
}
