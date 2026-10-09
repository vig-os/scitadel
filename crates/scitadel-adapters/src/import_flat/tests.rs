//! Tests for the flat-layout importer (ADR-007 §1 "Legacy data").
//!
//! Every test builds a real directory tree in a temp dir, runs the
//! importer against a real migrated SQLite database, and then reads the
//! rows back — so what is asserted is what a library would actually
//! hold, not what the planner intended.

use std::path::{Path, PathBuf};

use rusqlite::params;
use scitadel_core::models::{Paper, PaperId};
use scitadel_db::sqlite::Database;
use tempfile::TempDir;

use super::*;

/// Distinct bodies, so two files never collide by accident in a test
/// that is not about dedup.
const PDF_BYTES: &[u8] = b"%PDF-1.7\narticle body\n%%EOF\n";
const SI_BYTES: &[u8] = b"PK\x03\x04 supplementary workbook";
const TABLE_ONE: &[u8] = b"a,b\n1,2\n";
const TABLE_TWO: &[u8] = b"[{\"label\": \"Table 2\"}]";
const FIG_ONE: &[u8] = b"\x89PNG\r\n\x1a\nfigure one";
const FIG_TWO: &[u8] = b"\xff\xd8\xff\xe0 figure two";

/// `<paper-stem>` for the fixture paper. Hard-coded rather than derived
/// through `file_stem_for`, so a change in the stem rules shows up here
/// as a failure instead of silently moving the tree.
const STEM: &str = "arxiv_2005.07866";

/// A migrated database plus a flat tree to import into it.
///
/// The two are deliberately in *different* directories: the library root
/// (the DB's parent, where `blobs/` lives) is not the tree raid wrote,
/// which is the normal case and the one the `papers/<stem>` resolution
/// exists for.
struct Fixture {
    dir: TempDir,
    db: Database,
    /// The DB's parent directory — the library root and blob store.
    library_root: PathBuf,
    /// What gets handed to [`import_flat_tree`]; the paper directory
    /// lives at `<flat>/papers/<stem>`.
    flat: PathBuf,
    /// An unrelated directory outside the paper directory, for
    /// containment tests.
    outside: PathBuf,
    paper: Paper,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let library_root = root.join("library");
        std::fs::create_dir_all(&library_root).unwrap();
        let db = Database::open(&library_root.join("scitadel.db")).unwrap();
        db.migrate().unwrap();

        let mut paper = Paper::new("A paper with a flat directory");
        paper.id = PaperId::from("p-import");
        paper.arxiv_id = Some("2005.07866".into());
        assert_eq!(file_stem_for(&paper), STEM, "fixture stem assumption");

        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO papers (id, title, authors, year, created_at, updated_at)
             VALUES (?1, ?2, '[]', 2005, ?3, ?3)",
            params![
                paper.id.as_str(),
                paper.title.as_str(),
                "2026-01-02T03:04:05+00:00"
            ],
        )
        .unwrap();
        drop(conn);

        let flat = root.join("raid-export");
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(flat.join("papers").join(STEM)).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        Self {
            dir,
            db,
            library_root,
            flat,
            outside,
            paper,
        }
    }

    /// The paper directory the importer resolves to.
    fn paper_dir(&self) -> PathBuf {
        self.flat.join("papers").join(STEM)
    }

    /// Write a file relative to the paper directory, creating parents.
    fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = self.paper_dir().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn write_outside(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.outside.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn import(&self) -> ImportReport {
        import_flat_tree(&self.db, &self.paper, &self.flat).unwrap()
    }

    fn import_root(&self, root: &Path) -> Result<ImportReport, ImportError> {
        import_flat_tree(&self.db, &self.paper, root)
    }

    /// Every artefact row for the fixture paper, in a stable order.
    fn rows(&self) -> Vec<Row> {
        let conn = self.db.conn().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, version, locator, sha256, format, access_status, route,
                        access_basis, label, caption, imported_from, retrieved_at, missing_on_disk
                 FROM artefacts WHERE paper_id = ?1 ORDER BY kind, locator",
            )
            .unwrap();
        stmt.query_map(params![self.paper.id.as_str()], |r| {
            Ok(Row {
                id: r.get(0)?,
                kind: r.get(1)?,
                version: r.get(2)?,
                locator: r.get(3)?,
                sha256: r.get(4)?,
                format: r.get(5)?,
                access_status: r.get(6)?,
                route: r.get(7)?,
                access_basis: r.get(8)?,
                label: r.get(9)?,
                caption: r.get(10)?,
                imported_from: r.get(11)?,
                retrieved_at: r.get(12)?,
                missing_on_disk: r.get(13)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    fn blobs(&self) -> Vec<(String, i64, Option<String>, String)> {
        let conn = self.db.conn().unwrap();
        let mut stmt = conn
            .prepare("SELECT sha256, bytes, mime, rel_path FROM blobs ORDER BY sha256")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn acquisition_state(&self) -> Vec<(String, String, String, Option<String>, Option<String>)> {
        let conn = self.db.conn().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT kind, locator, status, reason, drop_path FROM acquisition_state
                 WHERE paper_id = ?1 ORDER BY kind, locator",
            )
            .unwrap();
        stmt.query_map(params![self.paper.id.as_str()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    /// Run a `SELECT COUNT(*)` over the fixture paper. The SQL must bind
    /// `paper_id` as its single parameter.
    fn count(&self, sql: &str) -> i64 {
        let conn = self.db.conn().unwrap();
        conn.query_row(sql, params![self.paper.id.as_str()], |r| r.get(0))
            .unwrap()
    }

    /// Read the stored copy of a blob by its digest.
    fn stored_bytes(&self, sha: &str) -> Vec<u8> {
        let rel: String = {
            let conn = self.db.conn().unwrap();
            conn.query_row(
                "SELECT rel_path FROM blobs WHERE sha256 = ?1",
                params![sha],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(!rel.starts_with('/'), "stored paths are root-relative");
        std::fs::read(self.library_root.join(rel)).unwrap()
    }
}

/// One `artefacts` row, as the tests want to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: String,
    kind: String,
    version: String,
    locator: String,
    sha256: Option<String>,
    format: Option<String>,
    access_status: String,
    route: String,
    access_basis: String,
    label: Option<String>,
    caption: Option<String>,
    imported_from: Option<String>,
    retrieved_at: String,
    missing_on_disk: i64,
}

impl Row {
    fn short_id(&self) -> &str {
        &self.id[..8]
    }
}

/// The tree from ADR-007 §1 "Legacy data", complete: one PDF, one SI
/// file, two tables, and two figures with captions.
fn full_tree(fx: &Fixture) {
    fx.write("fulltext.pdf", PDF_BYTES);
    fx.write("si/Supplementary Table 1.xlsx", SI_BYTES);
    fx.write("tables/table-1.csv", TABLE_ONE);
    fx.write("tables/table-2.json", TABLE_TWO);
    fx.write("figures/fig1.png", FIG_ONE);
    fx.write(
        "figures/fig1.caption.txt",
        b"Figure 1: the reaction scheme.",
    );
    fx.write("figures/fig2.jpg", FIG_TWO);
    fx.write(
        "figures/fig2.caption.txt",
        b"Figure 2: the energy landscape.",
    );
}

/// ADR-007 §1's acceptance criterion: a full flat tree imports one row
/// per file, filed under the kind its slot implies.
#[test]
fn a_full_flat_tree_imports_every_file() {
    let fx = Fixture::new();
    full_tree(&fx);

    let report = fx.import();

    assert_eq!(report.count("fulltext_pdf"), 1, "{report:?}");
    assert_eq!(report.count("si"), 1, "{report:?}");
    assert_eq!(report.count("table"), 2, "{report:?}");
    assert_eq!(report.count("figure"), 2, "{report:?}");
    assert_eq!(report.total(), 6, "one row per file: {report:?}");
    assert_eq!(fx.rows().len(), 6, "and one row in the table");

    let rows = fx.rows();
    let pdf = row_of(&rows, "fulltext_pdf");
    assert_eq!(pdf.locator, "", "full text carries no locator");
    assert_eq!(pdf.format.as_deref(), Some("pdf"));
    assert_eq!(pdf.access_status, "full_text");
    assert_eq!(
        pdf.version, "unknown",
        "a flat file has no recorded version"
    );
    assert_eq!(pdf.route, "import_flat");
    assert_eq!(pdf.access_basis, "manual");
    assert!(pdf.sha256.is_some());

    let si = row_with(&rows, "si", "supplementary-table-1");
    assert_eq!(si.format.as_deref(), Some("xlsx"));
    assert_eq!(si.label.as_deref(), Some("Supplementary Table 1"));
    assert_eq!(
        si.access_status, "unknown",
        "nothing vouched for the content of an SI file"
    );

    // Both figures carried a caption, and it landed on the row rather
    // than becoming an artefact of its own.
    let fig1 = row_with(&rows, "figure", "fig1");
    assert_eq!(
        fig1.caption.as_deref(),
        Some("Figure 1: the reaction scheme.")
    );
    assert_eq!(fig1.format.as_deref(), Some("png"));
    assert_eq!(
        row_with(&rows, "figure", "fig2").format.as_deref(),
        Some("jpg")
    );
    assert!(
        !rows
            .iter()
            .any(|r| r.kind == "figure" && r.format.as_deref() == Some("txt")),
        "a caption is metadata, not a second artefact"
    );

    // One blob per file, each readable at its content-addressed path.
    assert_eq!(report.blobs, 6);
    assert_eq!(fx.blobs().len(), 6);
    assert_eq!(fx.stored_bytes(pdf.sha256.as_deref().unwrap()), PDF_BYTES);
    assert_eq!(fx.stored_bytes(si.sha256.as_deref().unwrap()), SI_BYTES);

    // The full text is present, so nothing is missing.
    assert!(report.gaps.is_empty(), "{report:?}");
    assert!(fx.acquisition_state().is_empty());
}

/// Two tables under one work collide on
/// `UNIQUE (paper_id, kind, version, locator)` unless their locators
/// differ — so the locator is what lets them coexist.
#[test]
fn two_tables_under_one_paper_do_not_collide() {
    let fx = Fixture::new();
    fx.write("tables/table-1.csv", TABLE_ONE);
    fx.write("tables/table-2.csv", TABLE_TWO);

    let report = fx.import();

    assert_eq!(report.count("table"), 2);
    let rows = fx.rows();
    assert_eq!(rows.len(), 2, "both tables survive: {rows:?}");
    assert_ne!(
        row_with(&rows, "table", "table-1").short_id(),
        row_with(&rows, "table", "table-2").short_id(),
        "distinct locators mean distinct rows with distinct derived ids"
    );
    assert_ne!(
        row_with(&rows, "table", "table-1").sha256,
        row_with(&rows, "table", "table-2").sha256,
        "and each keeps its own bytes"
    );

    // A third file that normalises onto one of those locators would
    // collide, so the importer disambiguates rather than letting the
    // second silently overwrite the first.
    fx.write("tables/Table 1.json", b"[]");
    let report = fx.import();
    assert_eq!(report.count("table"), 3, "the third file is not dropped");
    let rows = fx.rows();
    let locators: Vec<&str> = rows
        .iter()
        .filter(|r| r.kind == "table")
        .map(|r| r.locator.as_str())
        .collect();
    assert_eq!(locators, ["table-1", "table-1.json", "table-2"]);
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM artefacts
             WHERE paper_id = ?1 AND kind = 'table' AND locator = 'table-1'",
        ),
        1,
        "the UNIQUE key still holds exactly one row per locator"
    );
}

/// #287: a caption written by whoever produced the file cannot reach a terminal.
///
/// The end-to-end version of the boundary, and the one that proves the string is
/// reachable rather than theoretical: a real `.caption.txt` sidecar is written into
/// a real flat tree, imported by the real importer, read back through the real
/// reader, and rendered. Every shape a hostile caption can take is in the one file —
/// a CSI that repaints, an OSC 8 that makes it a link, an OSC 0 that renames the
/// window, a bidi override that reverses how it looks — and every one of them is
/// gone from the rendered form while the words survive.
///
/// The mirror is asserted separately, because the two requirements are different and
/// both real: what a reader sees must be safe, and what the provenance document
/// records must be faithful.
#[test]
fn a_hostile_caption_sidecar_is_neutralised_before_it_can_be_rendered() {
    use scitadel_core::untrusted::Provenance;

    let fx = Fixture::new();
    let hostile = concat!(
        "Figure 9: the uptake curve for cohort A\n",
        "\u{1b}[31m\u{1b}[2J\u{1b}[H",
        "\u{1b}]8;;https://attacker.example/\u{1b}\\see the raw data\u{1b}]8;;\u{1b}\\",
        "\u{1b}]0;window title\u{7}",
        "\u{202e}mirrored",
    );
    fx.write("figures/fig9.caption.txt", hostile.as_bytes());

    let report = fx.import();
    assert_eq!(report.count("figure"), 1, "{report:?}");

    let conn = fx.db.conn().unwrap();
    let stored: Vec<(String, Option<String>)> = {
        let mut stmt = conn
            .prepare("SELECT locator, caption FROM artefacts WHERE paper_id = ?1")
            .unwrap();
        let rows = stmt
            .query_map(params![fx.paper.id.as_str()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    };
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].1.as_deref(),
        Some(hostile),
        "the importer stores the caption verbatim — a mirror that quietly cleaned \
         it would be lying about what the file said"
    );

    let artefacts =
        scitadel_db::sqlite::read_artefacts_for_paper(&conn, fx.paper.id.as_str()).unwrap();
    let caption = artefacts[0].caption_text().expect("a caption");
    assert_eq!(caption.provenance(), Provenance::PublisherSupplied);
    assert_eq!(caption.as_str(), hostile);

    let rendered = caption.rendered().into_owned();
    assert!(
        !rendered.contains('\u{1b}'),
        "no escape byte survives: {rendered:?}"
    );
    for needle in ["attacker.example", "window title", "\u{202e}", "[2J"] {
        assert!(!rendered.contains(needle), "{needle} must not survive");
    }
    assert!(
        rendered.starts_with("Figure 9: the uptake curve for cohort A see the raw data mirrored"),
        "the words survive, in order, on one line: {rendered:?}"
    );
    assert!(
        !rendered.contains('\n'),
        "an embedded newline is collapsed, so a caption cannot reflow a table row"
    );

    // And `Display` — what `{}` reaches — is that same neutralised form, so a
    // future render site cannot get this wrong by accident.
    assert_eq!(caption.to_string(), rendered);
}

/// ADR-007 §1 "Artefact rules": a figure can be a *reference* —
/// `sha256 IS NULL` with a caption — because raid's digitisation queue
/// needs the caption and the location, not the image.
#[test]
fn a_figure_reference_without_bytes_is_recorded() {
    let fx = Fixture::new();
    let caption = fx.write(
        "figures/fig9.caption.txt",
        b"Figure 9: no image was archived.",
    );

    let report = fx.import();

    assert_eq!(report.count("figure"), 1, "{report:?}");
    let rows = fx.rows();
    let fig = row_with(&rows, "figure", "fig9");
    assert_eq!(fig.sha256, None, "there are no bytes to hash");
    assert_eq!(
        fig.caption.as_deref(),
        Some("Figure 9: no image was archived.")
    );
    assert_eq!(fig.label.as_deref(), Some("fig9"));
    assert_eq!(fig.format, None);
    assert_eq!(
        fig.missing_on_disk, 0,
        "nothing is missing — the caption was the artefact"
    );
    assert_eq!(fig.route, "import_flat");
    assert_eq!(
        fig.imported_from.as_deref(),
        caption.to_str(),
        "where the information came from is recorded"
    );
    assert!(
        fx.blobs().is_empty(),
        "a reference with no bytes adds no blob: sha256 must reference blobs"
    );
    assert_eq!(report.blobs, 0);
}

/// Every row says which file it came from — that is what lets a moved
/// library still name the provenance a datasheet cites.
#[test]
fn imported_from_records_the_original_path() {
    let fx = Fixture::new();
    full_tree(&fx);

    fx.import();

    let rows = fx.rows();
    assert_eq!(rows.len(), 6);
    for row in &rows {
        let imported = row
            .imported_from
            .as_deref()
            .unwrap_or_else(|| panic!("no imported_from on {row:?}"));
        let path = Path::new(imported);
        assert!(path.is_absolute(), "{imported} is not absolute");
        assert!(path.is_file(), "{imported} does not exist");
        assert!(
            path.starts_with(fx.paper_dir()),
            "{imported} is outside the paper directory"
        );
    }
    let expected: BTreeSet<String> = [
        "fulltext.pdf",
        "si/Supplementary Table 1.xlsx",
        "tables/table-1.csv",
        "tables/table-2.json",
        "figures/fig1.png",
        "figures/fig2.jpg",
    ]
    .iter()
    .map(|rel| fx.paper_dir().join(rel).to_string_lossy().into_owned())
    .collect();
    let actual: BTreeSet<String> = rows
        .iter()
        .map(|r| r.imported_from.clone().unwrap())
        .collect();
    assert_eq!(actual, expected, "each row names its own file");
}

/// ADR-007 §1 "Legacy data": the import is idempotent. A second run
/// derives the same ids, finds the same hashes, finds the blobs already
/// in place, and writes nothing at all.
#[test]
fn importing_the_same_tree_twice_changes_nothing() {
    let fx = Fixture::new();
    full_tree(&fx);

    let first = fx.import();
    let rows = fx.rows();
    let blobs = fx.blobs();
    let state = fx.acquisition_state();

    let second = fx.import();
    assert_eq!(second.counts, first.counts);
    assert_eq!(fx.rows(), rows, "a second import changes no artefact row");
    assert_eq!(fx.blobs(), blobs, "and adds no blob");
    assert_eq!(fx.acquisition_state(), state);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM artefacts WHERE paper_id = ?1"),
        6,
        "no duplicate rows"
    );

    // A third run, after a `migrate()` in between — the path a real
    // process takes — is a no-op too.
    fx.db.migrate().unwrap();
    fx.import();
    assert_eq!(fx.rows(), rows);
    assert_eq!(fx.blobs(), blobs);
}

/// An empty directory is a fact about the library, not an error — and
/// the absent full text is recorded rather than skipped.
#[test]
fn an_empty_directory_produces_no_artefacts_but_does_not_error() {
    let fx = Fixture::new();
    let empty = fx.dir.path().join("nothing-here");
    std::fs::create_dir_all(&empty).unwrap();

    let report = fx.import_root(&empty).expect("an empty directory imports");

    assert_eq!(report.total(), 0);
    assert!(report.counts.is_empty());
    assert!(report.refused.is_empty());
    assert!(report.unrecognised.is_empty());
    assert!(fx.rows().is_empty(), "nothing was fabricated");
    assert!(fx.blobs().is_empty());

    // The gap is recorded instead, so `coverage` can report it.
    assert_eq!(report.gaps.len(), 1, "{report:?}");
    assert_eq!(report.gaps[0].kind, "fulltext");
    assert_eq!(
        report.gaps[0].drop_path,
        empty.join("fulltext.pdf"),
        "the gap names where the file belongs"
    );
    let state = fx.acquisition_state();
    assert_eq!(state.len(), 1);
    assert_eq!(state[0].0, "fulltext");
    assert_eq!(state[0].2, "pending");
    let drop_path = empty.join("fulltext.pdf");
    assert_eq!(state[0].4.as_deref(), drop_path.to_str());
    assert!(
        state[0]
            .3
            .as_deref()
            .is_some_and(|r| r.contains("flat tree")),
        "the reason names what was looked for: {state:?}"
    );

    // A root that does not exist at all *is* an error — that is a
    // mistake by the caller, not a fact about the library.
    assert!(matches!(
        fx.import_root(&fx.dir.path().join("no-such-dir")),
        Err(ImportError::RootNotFound { .. })
    ));
}

/// A file that resolves outside the tree being imported is refused and
/// never read. This is a containment boundary, so it is tested as one.
#[test]
fn a_file_outside_the_root_is_not_imported() {
    let fx = Fixture::new();
    fx.write("tables/table-1.csv", TABLE_ONE);
    let secret = fx.write_outside("secret.csv", b"col from outside the tree");
    // A symlink inside the tree pointing out of it.
    std::os::unix::fs::symlink(&secret, fx.paper_dir().join("tables/escape.csv")).unwrap();
    // A symlinked *directory* pointing out of it.
    let sneaky_dir = fx.outside.join("sneaky");
    std::fs::create_dir_all(&sneaky_dir).unwrap();
    std::fs::write(sneaky_dir.join("sneaky.csv"), b"also outside").unwrap();
    std::os::unix::fs::symlink(&sneaky_dir, fx.paper_dir().join("tables/linked")).unwrap();

    let report = fx.import();

    // Both escaping entries are refused outright — the symlinked file and
    // the symlinked directory, which is therefore never descended into.
    let mut refused: Vec<String> = report
        .refused
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    refused.sort();
    let mut expected: Vec<String> = [
        fx.paper_dir().join("tables/escape.csv"),
        fx.paper_dir().join("tables/linked"),
    ]
    .iter()
    .map(|p| p.to_string_lossy().into_owned())
    .collect();
    expected.sort();
    assert_eq!(
        refused, expected,
        "every escaping entry is named, not silently dropped"
    );
    let rows = fx.rows();
    assert_eq!(rows.len(), 1, "only the real table was imported: {rows:?}");
    assert_eq!(rows[0].locator, "table-1");
    assert!(
        !rows
            .iter()
            .any(|r| r.locator.contains("escape") || r.locator.contains("sneaky")),
        "nothing from outside the root became a row"
    );
    assert_eq!(fx.blobs().len(), 1, "and nothing outside was stored");
    assert_eq!(
        fx.stored_bytes(rows[0].sha256.as_deref().unwrap()),
        TABLE_ONE,
        "the stored bytes are the ones inside the tree"
    );

    // The containment check itself. Both a `..` path whose target exists
    // (resolved by `canonicalize`) and one that does not (resolved
    // lexically) must be refused — an escape is an escape whether or not
    // there is anything to steal.
    let root = fx.paper_dir();
    assert!(within_root(&root, &root.join("tables/table-1.csv")));
    assert!(!within_root(&root, &root.join("tables/../../secret.csv")));
    assert!(
        secret.exists() && !within_root(&root, &secret),
        "a real file outside the root is refused, not merely an unresolvable path"
    );
    assert!(
        !within_root(&root, root.parent().unwrap()),
        "a sibling with a shared prefix is not inside"
    );
    assert_eq!(
        lexical_normalise(Path::new("/a/b/../c/./d")),
        PathBuf::from("/a/c/d")
    );
    assert_eq!(
        lexical_normalise(Path::new("/../..")),
        PathBuf::from("/"),
        "a rooted path cannot climb above its root"
    );
}

/// A gap recorded by an earlier run is retracted once the file it named
/// turns up — leaving it would tell `coverage` the full text is both
/// wanted and held.
#[test]
fn a_recorded_gap_is_retracted_when_the_file_appears() {
    let fx = Fixture::new();
    let tree = fx.outside.join("tree");
    std::fs::create_dir_all(&tree).unwrap();

    let first = fx.import_root(&tree).unwrap();
    assert_eq!(first.gaps.len(), 1);
    assert_eq!(fx.acquisition_state().len(), 1);

    // The full text lands where the gap said it should.
    let pdf = tree.join("fulltext.pdf");
    std::fs::write(&pdf, PDF_BYTES).unwrap();

    let second = fx.import_root(&tree).unwrap();
    assert!(second.gaps.is_empty(), "the gap is closed: {second:?}");
    assert_eq!(second.count("fulltext_pdf"), 1);
    assert!(
        fx.acquisition_state().is_empty(),
        "and the row saying we wanted it is gone"
    );
    let rows = fx.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].imported_from.as_deref(),
        pdf.to_str(),
        "the row records the file it came from"
    );
}

/// A file we cannot hash is recorded with its path and no hash rather
/// than skipped — ADR-007's own rule for a recorded path with no usable
/// bytes. A dangling symlink inside the tree is the honest way to get
/// one: its resolved path is inside the root, and reading it fails.
#[test]
fn a_file_we_cannot_read_is_recorded_as_missing_on_disk() {
    let fx = Fixture::new();
    let dangling = fx.outside.join("never-written.csv");
    std::fs::create_dir_all(fx.paper_dir().join("tables")).unwrap();
    std::os::unix::fs::symlink(&dangling, fx.paper_dir().join("tables/table-1.csv")).unwrap();
    assert!(!dangling.exists(), "the symlink dangles");

    let report = fx.import();

    assert_eq!(report.count("table"), 1, "{report:?}");
    let rows = fx.rows();
    assert_eq!(rows[0].locator, "table-1");
    assert_eq!(rows[0].sha256, None);
    assert_eq!(rows[0].missing_on_disk, 1);
    assert!(
        rows[0].imported_from.is_some(),
        "the path is still recorded"
    );
    assert!(fx.blobs().is_empty(), "no blob for bytes nobody can read");
    assert!(
        !report
            .refused
            .contains(&fx.paper_dir().join("tables/table-1.csv")),
        "a dangling symlink inside the root is not an escape attempt"
    );
}

/// A directory where a file is expected is not a table: it is walked as
/// a directory, and since its name is not a slot, nothing under it is
/// imported and no row is invented.
#[test]
fn a_directory_in_a_slot_is_not_imported_as_an_artefact() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.paper_dir().join("tables/table-1.csv")).unwrap();
    std::fs::write(
        fx.paper_dir().join("tables/table-1.csv/nested.png"),
        FIG_ONE,
    )
    .unwrap();

    let report = fx.import();

    assert_eq!(report.total(), 0, "{report:?}");
    assert!(fx.rows().is_empty());
    assert!(report.unrecognised.is_empty());
}

/// Files outside the `kind` vocabulary are left alone rather than
/// mislabelled, and reported so the operator knows they were seen.
#[test]
fn an_unrecognised_file_is_reported_and_left_in_place() {
    let fx = Fixture::new();
    // SI accepts anything; `tables/` and the full-text slot do not.
    fx.write("si/notes.docx", b"PK\x03\x04 si is unbounded");
    let notes = fx.write("tables/notes.docx", b"not a table serialisation");
    let docx = fx.write("fulltext.docx", b"not a full text");

    let report = fx.import();

    assert_eq!(report.count("si"), 1, "SI is unbounded: {report:?}");
    assert_eq!(report.count("table"), 0);
    assert_eq!(report.count("fulltext_pdf"), 0);
    assert_eq!(
        report.unrecognised.len(),
        2,
        "both non-vocabulary files are named: {:?}",
        report.unrecognised
    );
    assert!(report.unrecognised.contains(&notes));
    assert!(notes.exists(), "and no file is touched");
    assert!(docx.exists());
    // The full text is genuinely absent, so that is a recorded gap.
    assert_eq!(report.gaps.len(), 1, "{report:?}");
}

/// raid's single-directory spelling (`<slug>_SI_<name>.<ext>`) lands in
/// the same `si` kind, with the same locator the `si/` subdirectory
/// would have produced.
#[test]
fn raids_inline_supplement_spelling_is_imported_as_si() {
    let fx = Fixture::new();
    fx.write("fulltext.pdf", PDF_BYTES);
    fx.write(&format!("{STEM}_SI_Supplementary Table 1.xlsx"), SI_BYTES);

    let report = fx.import();

    assert_eq!(report.count("si"), 1, "{report:?}");
    let rows = fx.rows();
    let si = row_of(&rows, "si");
    assert_eq!(
        si.locator, "supplementary-table-1",
        "either spelling produces the same locator"
    );
}

/// An in-memory database has no library root, so it can hold no blobs —
/// `import-flat` says so instead of writing rows that reference nothing.
#[test]
fn an_in_memory_database_is_refused() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let mut paper = Paper::new("no root");
    paper.id = PaperId::from("p-mem");
    paper.arxiv_id = Some("2005.07866".into());

    let err = import_flat_tree(&db, &paper, Path::new("/tmp")).unwrap_err();
    assert!(matches!(err, ImportError::NoLibraryRoot), "{err:?}");
    assert!(err.to_string().contains("file-backed"));
}

fn row_of<'a>(rows: &'a [Row], kind: &str) -> &'a Row {
    rows.iter()
        .find(|r| r.kind == kind)
        .unwrap_or_else(|| panic!("no {kind} row in {rows:?}"))
}

fn row_with<'a>(rows: &'a [Row], kind: &str, locator: &str) -> &'a Row {
    rows.iter()
        .find(|r| r.kind == kind && r.locator == locator)
        .unwrap_or_else(|| panic!("no {kind} row with locator {locator} in {rows:?}"))
}

#[test]
fn labels_normalise_into_stable_locators() {
    assert_eq!(normalise_label("table-1"), "table-1");
    assert_eq!(
        normalise_label("Supplementary Table 1"),
        "supplementary-table-1"
    );
    assert_eq!(normalise_label("fig1"), "fig1");
    assert_eq!(normalise_label("  Table  2  "), "table-2");
    assert_eq!(normalise_label("Table__2"), "table-2");
    assert_eq!(normalise_label("S1 (a)"), "s1-a");
    assert_eq!(normalise_label("___"), "___", "never empty");
    assert_eq!(
        normalise_label("\u{c9}"),
        "_",
        "non-ASCII is not a locator char"
    );
}

#[test]
fn the_decision_table_maps_each_slot() {
    let artefacts = [
        (Slot::FullText, "fulltext.pdf", "fulltext_pdf"),
        (Slot::FullText, "fulltext.html", "fulltext_html"),
        (Slot::FullText, "fulltext.xml", "fulltext_xml"),
        (Slot::FullText, "fulltext.nxml", "fulltext_xml"),
        (Slot::Si, "anything.xyz", "si"),
        (Slot::Table, "table-1.csv", "table"),
        (Slot::Figure, "fig1.png", "figure"),
    ];
    for (slot, name, expected) in artefacts {
        match classify(slot, name, STEM) {
            Classified::Artefact { kind, .. } => assert_eq!(kind, expected, "{slot:?} {name}"),
            other => panic!("{slot:?} {name} classified as {other:?}"),
        }
    }
    // Outside the vocabulary: reported, never guessed at.
    for (slot, name) in [
        (Slot::FullText, "fulltext.docx"),
        (Slot::FullText, "notes.csv"),
        (Slot::Table, "table-1.docx"),
        (Slot::Figure, "fig1.txt"),
    ] {
        assert_eq!(
            classify(slot, name, STEM),
            Classified::Unrecognised,
            "{slot:?} {name}"
        );
    }
    // The `<stem>.<ext>` spelling `download_paper` writes is a full text.
    assert!(matches!(
        classify(Slot::FullText, &format!("{STEM}.pdf"), STEM),
        Classified::Artefact {
            kind: "fulltext_pdf",
            ..
        }
    ));
    // raid's inline SI spelling.
    assert!(matches!(
        classify(Slot::FullText, "slug_SI_Supplementary Table 1.xlsx", "slug"),
        Classified::Artefact {
            slot: Slot::Si,
            kind: "si",
            ..
        }
    ));
    assert_eq!(
        classify(Slot::FullText, "meta.json", STEM),
        Classified::Deferred
    );
    assert_eq!(slot_for_dir("si"), Some(Slot::Si));
    assert_eq!(slot_for_dir("Tables"), Some(Slot::Table));
    assert_eq!(slot_for_dir("random"), None);
}
