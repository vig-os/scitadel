#![allow(deprecated)] // `Command::cargo_bin` is the stable assert_cmd entry.

//! End-to-end `scitadel attach`, `scitadel scan` and `scitadel gc`.
//!
//! Drives the real binary against a real database and reads the answers back from
//! each command's `--json` projection, so this covers the wiring the adapters' own
//! tests cannot: argument parsing, `--kind`'s value parser, id-prefix resolution,
//! the exit code a refusal produces, and `gc` exiting zero when there was nothing
//! to do.
//!
//! The refusal case is the one worth driving through the binary. ADR-007 §1
//! records that 7 of raid's 27 "SI" files were HTML landing pages — a silent
//! failure that a zero exit code would put straight back.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn cmd(db: &Path) -> Command {
    let mut c = Command::cargo_bin("scitadel").unwrap();
    c.env("SCITADEL_DB", db);
    c
}

fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("tests");
    p.push("fixtures");
    p.push("zotero-export.bib");
    p
}

/// `scitadel init --yes` plus a `.bib` import, returning the DB path and the one
/// paper id it created — the same seeding `tests/import_flat.rs` uses.
///
/// The database goes in `tmp/library/`, so the library root is that directory and
/// a file anywhere else in the tempdir is genuinely **outside** it. That is what
/// makes `imported_from`'s portability claim testable.
fn seeded(tmp: &TempDir) -> (PathBuf, String) {
    let db_path = tmp.path().join("library").join("scitadel.db");
    std::fs::create_dir_all(db_path.parent().expect("a parent")).expect("create the library");
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
    let out = cmd(&db_path)
        .args(["bib", "import"])
        .arg(fixture_path())
        .args(["--reader", "test-reader"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out);
    let short = stdout
        .lines()
        .find_map(|line| {
            let rest = line.trim_start().strip_prefix("created ")?;
            rest.split_whitespace().next().map(str::to_string)
        })
        .unwrap_or_else(|| panic!("no created paper in import output:\n{stdout}"));
    // The import prints the short form; every assertion below wants the full id,
    // because a mirror records the row's own primary key.
    let conn = rusqlite::Connection::open(&db_path).expect("open the db");
    let id: String = conn
        .query_row(
            "SELECT id FROM papers WHERE id LIKE ?1",
            [format!("{short}%")],
            |row| row.get(0),
        )
        .expect("the imported paper");
    (db_path, id)
}

/// A publisher's "your download is starting" page: what 7 of raid's 27 "SI" files
/// actually were.
fn landing_page() -> Vec<u8> {
    b"<!DOCTYPE html>\n<html lang=\"en\"><head><title>Preparing your download</title></head>\n\
      <body><p>Your file will be available shortly.</p></body></html>\n"
        .to_vec()
}

/// A real PDF whose `/Title` is the seeded work's own title, so the post-fetch
/// check corroborates instead of objecting. The title is the `.bib` fixture's.
fn real_si() -> Vec<u8> {
    b"%PDF-1.7\n/Title(Attention Is All You Need)\nbody\n%%EOF\n".to_vec()
}

/// Parse a command's `--json` stdout.
fn json_of(stdout: &[u8]) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}):\n{}",
            String::from_utf8_lossy(stdout)
        )
    })
}

/// Every `kind` in an `artefacts` projection, in the order the command printed it.
fn kinds(report: &Value) -> Vec<String> {
    let rows = report["files"].as_array().cloned().unwrap_or_default();
    rows.iter()
        .map(|row| row["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The refusal reasons in an `artefacts` projection, in order.
fn refusals(report: &Value) -> Vec<Option<String>> {
    let rows = report["files"].as_array().cloned().unwrap_or_default();
    rows.iter()
        .map(|row| {
            row["refused_because"]
                .as_str()
                .map(std::string::ToString::to_string)
        })
        .collect()
}

/// **The 7-of-27 case, through the binary.** An HTML page saved as
/// `S1.pdf`: refused, non-zero exit, the reason naming what the bytes are, and no
/// artefact row.
#[test]
fn attach_refuses_an_html_page_saved_as_an_si() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, id) = seeded(&tmp);
    let file = tmp.path().join("S1.pdf");
    std::fs::write(&file, landing_page()).unwrap();

    // The text mode, because an operator reads this: what it is, and why not.
    cmd(&db)
        .args(["attach", &id])
        .arg(&file)
        .args(["--kind", "si", "--label", "Supporting Information S1"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("NOT filed [bad_magic]"))
        .stdout(predicate::str::contains("markup document"))
        .stderr(predicate::str::contains("was not filed"));

    // And the machine mode, because an agent reads this — and it must carry the
    // same non-zero exit, or a script checking only the code sees a success.
    let out = cmd(&db)
        .args(["attach", &id])
        .arg(&file)
        .args([
            "--kind",
            "si",
            "--label",
            "Supporting Information S1",
            "--json",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&out);
    assert_eq!(report["file"]["refused_because"], "bad_magic");
    assert_eq!(report["file"]["kind"], "si");
    assert!(
        report["file"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("markup document")),
        "{report}"
    );

    // No artefact was created, and the blob store never saw the bytes.
    let out = cmd(&db)
        .args(["coverage", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&out);
    assert_eq!(
        report["missing_total"], 0,
        "no artefact was filed, so nothing is held: {report}"
    );
    assert!(
        !tmp.path().join("blobs").exists(),
        "and a landing page never enters the content-addressed store"
    );
}

/// The honest case: a real PDF is filed with `route = 'manual'` and
/// `access_status = 'unknown'` (ADR-007 §1 "Manual drop-ins"), the identity check
/// runs, and the mirror is written.
#[test]
fn attach_files_a_real_supplementary_file_and_writes_the_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, id) = seeded(&tmp);
    let file = tmp.path().join("S1.pdf");
    std::fs::write(&file, real_si()).unwrap();

    let out = cmd(&db)
        .args(["attach", &id])
        .arg(&file)
        .args([
            "--kind",
            "si",
            "--label",
            "Supporting Information S1",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&out);
    assert_eq!(report["file"]["kind"], "si");
    assert_eq!(report["file"]["locator"], "supporting-information-s1");
    assert_eq!(report["file"]["refused_because"], Value::Null);
    assert_eq!(
        report["identity"]["status"], "unverified",
        "ADR-007 §1's post-fetch check ran, and a PDF title alone corroborates nothing"
    );
    assert_eq!(
        report["inside_library"], false,
        "attaching from outside the library says so, because `imported_from` is an absolute path \
         that will not move with the library"
    );

    // The mirror sits in the work's own directory, next to what it describes.
    let mirror = PathBuf::from(report["manifest"]["path"].as_str().expect("a path"));
    assert!(mirror.exists(), "the mirror is at {}", mirror.display());
    let body: Value = serde_json::from_str(&std::fs::read_to_string(&mirror).unwrap()).unwrap();
    assert_eq!(body["work"]["paper_id"], id.as_str());
    assert_eq!(body["artefacts"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        body["artefacts"][0]["route"], "manual",
        "and it describes the row the database holds"
    );

    // `--kind` that contradicts the bytes is refused by name, which is what keeps
    // the magic-byte check from being bypassable through the command line.
    let html = tmp.path().join("page.html");
    std::fs::write(&html, landing_page()).unwrap();
    cmd(&db)
        .args(["attach", &id])
        .arg(&html)
        .args(["--kind", "fulltext_pdf"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("fulltext_html"))
        .stderr(predicate::str::contains("fulltext_pdf"));
}

/// `scan --dry-run` writes nothing, takes no lease, and names the file it would
/// refuse.
#[test]
fn scan_dry_run_writes_nothing_and_names_the_landing_page() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, id) = seeded(&tmp);
    let drop = tmp.path().join("drop-in");
    std::fs::create_dir_all(drop.join("si")).unwrap();
    std::fs::write(drop.join("si").join("S1.pdf"), landing_page()).unwrap();
    std::fs::write(drop.join("si").join("S2.pdf"), real_si()).unwrap();

    let out = cmd(&db)
        .args(["scan", "--paper", &id, "--root"])
        .arg(&drop)
        .args(["--dry-run", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&out);

    assert_eq!(kinds(&report), vec!["si", "si"], "both files were walked");
    assert_eq!(
        refusals(&report),
        vec![Some("bad_magic".to_string()), None],
        "the landing page is the one refusal"
    );
    assert!(report["manifest"].is_null(), "a plan writes no mirror");
    assert!(
        report["lease"].is_null(),
        "and takes no lease, so it cannot block a real run"
    );

    // Nothing was written: a dry run that moved a row would make its own report
    // unfalsifiable.
    assert!(
        !drop.join("manifest.json").exists(),
        "and no manifest anywhere"
    );
    let out = cmd(&db)
        .args(["coverage", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_of(&out)["held_total"], 0, "no artefact was filed");
}

/// The real run: the landing page is refused and reported, the real PDF is filed,
/// and a file that then disappears is flagged with its row kept.
#[test]
fn scan_files_the_real_file_refuses_the_landing_page_and_flags_a_vanished_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, id) = seeded(&tmp);
    let drop = tmp.path().join("drop-in");
    std::fs::create_dir_all(drop.join("si")).unwrap();
    std::fs::write(drop.join("si").join("S1.pdf"), landing_page()).unwrap();
    let real = drop.join("si").join("S2.pdf");
    std::fs::write(&real, real_si()).unwrap();

    let out = cmd(&db)
        .args(["scan", "--paper", &id, "--root"])
        .arg(&drop)
        .args(["--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&out);
    // The text mode says the same thing in prose, because a refusal that only
    // appears in a JSON field is one a human skips.
    cmd(&db)
        .args(["scan", "--paper", &id, "--root"])
        .arg(&drop)
        .assert()
        .success()
        .stdout(predicate::str::contains("Not filed (1)"))
        .stdout(predicate::str::contains("bad_magic"));
    assert_eq!(refusals(&report), vec![Some("bad_magic".to_string()), None]);
    assert!(report["manifest"]["written"].as_bool().expect("written"));

    // The real file disappears. Its row stays, flagged.
    std::fs::remove_file(&real).unwrap();
    let out = cmd(&db)
        .args(["scan", "--paper", &id, "--root"])
        .arg(&drop)
        .args(["--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&out);
    cmd(&db)
        .args(["scan", "--paper", &id, "--root"])
        .arg(&drop)
        .assert()
        .success()
        .stdout(predicate::str::contains("Gone from disk (1)"));
    assert_eq!(
        report["vanished"],
        serde_json::json!(["si::s2"]),
        "ADR-007 §1: the row stays and is flagged missing_on_disk = 1"
    );
    let still_held = cmd(&db)
        .args(["coverage", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&still_held);
    assert_eq!(
        report["held_total"], 0,
        "and `coverage` no longer counts it as held, which is the point of the flag"
    );
}

/// `gc --dry-run` collects nothing and says what it would; a real `gc` collects
/// the unreferenced blob. Both exit zero, because "the store is as it should be"
/// is a success either way — and the report says which of the two happened.
#[test]
fn gc_dry_run_deletes_nothing_and_gc_reports_what_it_collected() {
    let tmp = tempfile::tempdir().unwrap();
    let (db, _id) = seeded(&tmp);

    // Two blobs' worth of bytes on disk, and one referenced by an artefact. The
    // rows are written by hand because `attach` only ever creates referenced ones,
    // and the point is what happens to a blob nothing points at.
    let held = "aa".repeat(32);
    let orphan = "bb".repeat(32);
    let library_root = tmp.path().join("library");
    for digest in [&held, &orphan] {
        let store = library_root
            .join("blobs")
            .join(&digest[..2])
            .join(format!("{digest}.pdf"));
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        std::fs::write(&store, b"%PDF-1.7\nbody\n%%EOF\n").unwrap();
    }
    // `gc` reads its candidates from the `blobs` table, so the rows have to exist.
    // `attach` gives us that for the held one; the orphan needs a second artefact's
    // blob, which we make by attaching the same bytes twice under two locators and
    // then deleting one of the rows' reference by dropping the artefact.
    seed_blobs(&db, &[(&held, true), (&orphan, false)]);

    cmd(&db)
        .args(["gc", "--dry-run", "--min-age-hours", "0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry run"))
        .stdout(predicate::str::contains("Nothing was deleted"));

    let out = cmd(&db)
        .args(["gc", "--dry-run", "--min-age-hours", "0", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&out);
    assert!(report["dry_run"].as_bool().expect("dry_run"));
    assert_eq!(
        report["candidates"].as_array().map(|c| c
            .iter()
            .map(|row| row["sha256"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>()),
        Some(vec![orphan.clone()]),
        "only the unreferenced blob is a candidate"
    );
    assert_eq!(report["collected_rows"], 0);
    assert_eq!(report["removed_files"], 0);
    assert!(
        library_root
            .join("blobs")
            .join(&orphan[..2])
            .join(format!("{orphan}.pdf"))
            .exists(),
        "a dry run removes no file"
    );

    // The text run does the collecting.
    cmd(&db)
        .args(["gc", "--min-age-hours", "0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Collected (1), 20 bytes"))
        .stdout(predicate::str::contains(
            "1 row(s) deleted, 1 file(s) removed",
        ));

    // And a second run finds nothing left, which is what "collected" means.
    let out = cmd(&db)
        .args(["gc", "--min-age-hours", "0", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = json_of(&out);
    assert_eq!(report["candidates"], serde_json::json!([]));
    assert_eq!(report["collected_rows"], 0);
    assert_eq!(report["removed_files"], 0);
    assert_eq!(
        report["blobs_total"], 1,
        "the referenced blob is all that is left"
    );
    assert!(
        library_root
            .join("blobs")
            .join(&held[..2])
            .join(format!("{held}.pdf"))
            .exists(),
        "the blob an artefact names survives: a wrong deletion destroys the only copy of a file"
    );
    assert!(
        !library_root
            .join("blobs")
            .join(&orphan[..2])
            .join(format!("{orphan}.pdf"))
            .exists(),
        "and the unreferenced one is gone"
    );
}

/// Put the two seeded digests into `blobs`, marking one as referenced.
///
/// Every real writer puts a `blobs` row and the `artefacts` row that names it in
/// **one transaction** — which is the property `gc`'s safety argument rests on —
/// so no command can produce an unreferenced row, and a test that needs one has to
/// write it. It is written here, through the same connection the binary uses, with
/// the two shapes the code has: one row with an artefact pointing at it and one
/// without.
fn seed_blobs(db: &Path, rows: &[(&str, bool)]) {
    let library = scitadel_db::sqlite::Database::open(db).expect("open the library");
    let library_root = library
        .library_root()
        .expect("library root")
        .expect("a file-backed library");
    library.migrate().expect("migrated");
    let conn = library.conn().expect("conn");
    let paper_id: String = conn
        .query_row("SELECT id FROM papers LIMIT 1", [], |row| row.get(0))
        .expect("a paper");
    for (digest, referenced) in rows {
        let rel = format!("blobs/{}/{digest}.pdf", &digest[..2]);
        let bytes = std::fs::metadata(library_root.join(&rel)).map_or(0, |m| m.len() as i64);
        conn.execute(
            "INSERT OR REPLACE INTO blobs (sha256, bytes, mime, rel_path, created_at)
             VALUES (?1, ?2, 'application/pdf', ?3, '2020-01-01T00:00:00+00:00')",
            rusqlite::params![digest, bytes, rel],
        )
        .expect("insert the blob");
        if *referenced {
            conn.execute(
                "INSERT OR REPLACE INTO artefacts
                     (id, paper_id, kind, version, locator, sha256, format, access_status,
                      route, access_basis, retrieved_at, missing_on_disk)
                 VALUES (?1, ?2, 'fulltext_pdf', 'vor', '', ?3, 'pdf', 'full_text',
                         'manual', 'manual', '2020-01-01T00:00:00+00:00', 0)",
                rusqlite::params![
                    scitadel_db::sqlite::artefact_id(&paper_id, "fulltext_pdf", "vor", ""),
                    paper_id,
                    digest
                ],
            )
            .expect("insert the reference");
        }
    }
}
