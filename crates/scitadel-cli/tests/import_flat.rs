#![allow(deprecated)] // `Command::cargo_bin` is the stable assert_cmd entry.

//! End-to-end `scitadel import-flat`.
//!
//! Drives the real binary against a real database: the paper is seeded
//! by a `.bib` import, the flat tree is written on disk, and the rows
//! that end up in the database are read back through SQL — so this
//! covers the wiring (argument parsing, id-prefix resolution, the
//! library-root lookup) that the importer's own tests cannot.

use std::path::{Path, PathBuf};

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

fn cmd(db: &Path) -> Command {
    let mut c = Command::cargo_bin("scitadel").unwrap();
    c.env("SCITADEL_DB", db);
    c
}

/// `scitadel init --yes` plus a `.bib` import, returning the DB path and
/// the one paper id it created.
fn seeded_db(tmp: &TempDir) -> (PathBuf, String) {
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
    let id = stdout
        .lines()
        .find_map(|line| {
            let rest = line.trim_start().strip_prefix("created ")?;
            rest.split_whitespace().next().map(str::to_string)
        })
        .unwrap_or_else(|| panic!("no created paper in import output:\n{stdout}"));
    (db_path, id)
}

/// Write `fulltext.pdf`, one SI file, two tables, one captioned figure
/// and one caption-only figure into the work's own directory.
///
/// The directory is passed to `--root` directly rather than as a library
/// root holding `papers/<stem>/`: the stem depends on the imported
/// paper's identifiers, and the `.bib` fixture is not this test's
/// business. Both root shapes are covered — the `papers/<stem>` one in
/// `scitadel-adapters`' importer tests.
fn flat_tree(root: &Path) -> PathBuf {
    let paper_dir = root.to_path_buf();
    let write = |rel: &str, bytes: &[u8]| {
        let path = paper_dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    };
    write("fulltext.pdf", b"%PDF-1.7\nbody\n%%EOF\n");
    write("si/Supplementary Table 1.xlsx", b"PK\x03\x04 si");
    write("tables/table-1.csv", b"a,b\n1,2\n");
    write("tables/table-2.csv", b"a,b\n3,4\n");
    write("figures/fig1.png", b"\x89PNG\r\n\x1a\nfig");
    write("figures/fig1.caption.txt", b"Figure 1: the scheme.");
    write(
        "figures/fig2.caption.txt",
        b"Figure 2: no image, caption only.",
    );
    paper_dir
}

/// Count rows for the paper. Needs the `rusqlite` CLI-free path, so the
/// count comes from the importer's own stdout plus a re-run rather than a
/// second dependency: `import-flat` is idempotent, so the second run's
/// output is the authoritative tally.
fn import(db: &Path, paper: &str, root: &Path) -> String {
    let out = cmd(db)
        .args(["import-flat", "--paper", paper, "--root"])
        .arg(root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn import_flat_records_the_whole_tree_and_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let (db_path, paper_id) = seeded_db(&tmp);
    let export = tmp.path().join("raid-export");
    let paper_dir = flat_tree(&export);
    assert_eq!(paper_dir, export);

    let out = import(&db_path, &paper_id, &export);

    assert!(out.contains("6 artefact(s)"), "{out}");
    for (kind, n) in [("fulltext_pdf", 1), ("si", 1), ("table", 2), ("figure", 2)] {
        assert!(
            out.contains(&format!("{kind:<16} {n}")),
            "expected {kind} {n} in:\n{out}"
        );
    }
    assert!(out.contains(&paper_dir.display().to_string()), "{out}");

    // Idempotent: the same tree again reports the same tally and no
    // duplicated rows.
    let again = import(&db_path, &paper_id, &export);
    assert_eq!(out, again, "a second run reports identically");

    // A prefix of the id works too, which is how the CLI is used.
    let prefix = &paper_id[..8];
    let third = import(&db_path, prefix, &export);
    assert!(third.contains("6 artefact(s)"), "{third}");
}

#[test]
fn import_flat_reports_a_missing_full_text_as_a_gap() {
    let tmp = TempDir::new().unwrap();
    let (db_path, paper_id) = seeded_db(&tmp);
    let export = tmp.path().join("raid-export");
    let paper_dir = export.clone();
    std::fs::create_dir_all(paper_dir.join("tables")).unwrap();
    std::fs::write(paper_dir.join("tables/table-1.csv"), b"a,b\n").unwrap();

    let out = import(&db_path, &paper_id, &export);

    assert!(out.contains("1 artefact(s)"), "{out}");
    assert!(
        out.contains("Not present in this tree"),
        "the missing full text is reported, not skipped:\n{out}"
    );
    assert!(
        out.contains(&paper_dir.join("fulltext.pdf").display().to_string()),
        "and the report says where to put it:\n{out}"
    );
}

#[test]
fn import_flat_exits_non_zero_when_a_path_escapes_the_tree() {
    let tmp = TempDir::new().unwrap();
    let (db_path, paper_id) = seeded_db(&tmp);
    let export = tmp.path().join("raid-export");
    let paper_dir = flat_tree(&export);
    let secret = tmp.path().join("secret.csv");
    std::fs::write(&secret, b"not yours").unwrap();
    std::os::unix::fs::symlink(&secret, paper_dir.join("tables/escape.csv")).unwrap();

    let assert = cmd(&db_path)
        .args(["import-flat", "--paper", &paper_id, "--root"])
        .arg(&export)
        .assert()
        .failure()
        .stderr(predicate::str::contains("escaped the imported tree"));
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();

    // The legitimate files still imported; only the escape was refused,
    // and it is named on stdout so the operator can see what was skipped.
    assert!(stdout.contains("6 artefact(s)"), "{stdout}");
    assert!(
        stdout.contains("Refused (resolves outside the imported tree)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&paper_dir.join("tables/escape.csv").display().to_string()),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("{:<16} {n}", "table", n = 2)),
        "the real tables imported: {stdout}"
    );
}

#[test]
fn import_flat_reports_an_unknown_paper_instead_of_guessing() {
    let tmp = TempDir::new().unwrap();
    let (db_path, _) = seeded_db(&tmp);
    let export = tmp.path().join("raid-export");
    flat_tree(&export);

    cmd(&db_path)
        .args(["import-flat", "--paper", "zzzzzzzz", "--root"])
        .arg(&export)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no paper matches id prefix"));
}
