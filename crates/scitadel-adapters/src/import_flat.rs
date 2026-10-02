//! ADR-007 §1 "Legacy data": the flat-layout importer.
//!
//! raid — and scitadel's own pre-0.9 `papers/` directory — laid a work
//! out as a directory of files named after the work, with no database
//! rows behind them. That layout is what this module reads:
//!
//! ```text
//! <root>/papers/<paper-stem>/fulltext.pdf
//! <root>/papers/<paper-stem>/fulltext.html
//! <root>/papers/<paper-stem>/si/Supplementary Table 1.xlsx
//! <root>/papers/<paper-stem>/tables/table-1.csv
//! <root>/papers/<paper-stem>/figures/fig1.png
//! <root>/papers/<paper-stem>/figures/fig1.caption.txt
//! ```
//!
//! `<paper-stem>` comes from
//! [`file_stem_for`](crate::download::file_stem_for) — the same DOI →
//! `arxiv_` → `openalex_` → UUID ladder `find_cached_file` and
//! `download_paper` already use, so this importer and the downloader
//! agree on what a work's directory is called.
//!
//! ## The decision table
//!
//! | slot | file | `kind` | `locator` | `format` |
//! |---|---|---|---|---|
//! | paper dir | `fulltext.pdf` | `fulltext_pdf` | `''` | `pdf` |
//! | paper dir | `fulltext.html` / `.htm` / `.xhtml` | `fulltext_html` | `''` | `html` |
//! | paper dir | `fulltext.xml` / `.nxml` / `.jats` | `fulltext_xml` | `''` | `xml` / `jats` |
//! | paper dir | `<stem>.pdf` / `.html` … | as above | `''` | as above |
//! | paper dir | `<stem>_SI_<name>.<ext>` (raid) | `si` | `<name>` normalised | `<ext>` |
//! | `si/` | anything | `si` | stem normalised | `<ext>` |
//! | `tables/` | `csv`, `tsv`, `json`, `xlsx`, `xls`, `ods` | `table` | stem normalised | `<ext>` |
//! | `figures/` | `png`, `jpg`, `jpeg`, `gif`, `svg`, `webp`, `tif`, `tiff`, `bmp`, `avif`, `eps`, `pdf` | `figure` | stem normalised | `<ext>` |
//! | `figures/` | `<id>.caption.txt`, no image | `figure` (`sha256 IS NULL`) | `<id>` normalised | `NULL` |
//! | `tables/` | `<id>.caption.txt`, no table | *(nothing — see below)* | | |
//!
//! Every row gets `version = 'unknown'` (these files predate version
//! tracking, and ADR-007 §1 "Have" trusts `unknown` on `import_flat`
//! rows for exactly that reason), `access_basis = 'manual'`,
//! `route = 'import_flat'`, and `imported_from` = the absolute path it
//! came from.
//!
//! `access_status` is `full_text` for the full-text slot — those bytes
//! *are* the full text — and `unknown` for `si` / `table` / `figure`,
//! because no machine classified them and the column has no better
//! value.
//!
//! An extension outside the vocabulary gets **no** row and a `warn!`
//! naming the path; the file is left untouched. A table caption with no
//! table file is likewise reported rather than filed: migration 013
//! allows a NULL `sha256` only for a *figure* reference.
//!
//! ## Losslessness
//!
//! * **Every file gets a row**, with `imported_from` = the absolute path
//!   it came from, so a published datasheet can still cite where a file
//!   lived after the library moves.
//! * **Every row gets a locator.** Two tables under one work collide on
//!   `UNIQUE (paper_id, kind, version, locator)` unless their locators
//!   differ, so `locator` is a normalised label (`table-1`,
//!   `supplementary-table-1`) and the human-readable original is kept in
//!   `label`.
//! * **A missing file is a fact, not a skip.** We do not fabricate a
//!   full-text row for a `fulltext.pdf` that was never there; we record
//!   an `acquisition_state` row naming what is wanted, so `coverage` can
//!   report the gap.
//!
//! ## Two phases, deliberately
//!
//! Same split as the legacy backfill in `scitadel-db`: read, hash and
//! copy into `blobs/` with **no write lock held**, then one short
//! transaction for the rows. A failure in phase one leaves the database
//! untouched. The blob store, the artefact id and the full-text
//! extension table all come from `scitadel-db`, so the same bytes can
//! never get two ids or two store paths, and this importer cannot
//! disagree with the backfill about what a `.pdf` is.
//!
//! ## Path containment
//!
//! An imported tree is untrusted input. Every entry is canonicalised and
//! checked against the canonicalised paper directory before it is read;
//! a `..` component or a symlink that resolves outside is **refused**,
//! named in [`ImportReport::refused`], and never hashed. Symlinked
//! directories are never descended into.
//!
//! ## Idempotence
//!
//! Re-running over an unchanged tree writes nothing: artefact ids are
//! derived from the UNIQUE key, `retrieved_at` comes from each file's
//! own mtime rather than the clock, and [`WriteMode::Reconcile`] updates
//! a row only where a value actually differs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use scitadel_core::models::Paper;
use scitadel_db::sqlite::{
    ACCESS_BASIS_MANUAL, ArtefactWrite, BlobWrite, Database, FULLTEXT_LOCATOR, ROUTE_IMPORT_FLAT,
    StateWrite, VERSION_UNKNOWN, WriteMode, blob_rel_path, file_extension, fulltext_kind,
    hash_file, store_blob,
};

use crate::download::file_stem_for;
use crate::error::AdapterError;

/// How deep the walk may go below the paper directory. The flat layout
/// is two levels deep; this only exists so a pathological tree cannot
/// turn an import into an unbounded crawl.
const MAX_DEPTH: usize = 8;

/// The filename the flat layout gives the full text, and therefore the
/// `drop_path` a recorded gap points at.
const FULLTEXT_FILENAME: &str = "fulltext.pdf";

/// Import failures. A file-level problem is never one of these — an
/// unreadable file becomes a flagged row and a refused path becomes a
/// line in the report — so one bad file cannot abort a whole library.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(
        "import-flat needs a file-backed library: an in-memory database has no \
         root, so it can hold no blobs (ADR-007 §1 \"Storage\")"
    )]
    NoLibraryRoot,
    #[error("no flat-layout directory for this paper under {root}")]
    RootNotFound { root: PathBuf },
    #[error(transparent)]
    Db(#[from] scitadel_db::error::DbError),
}

impl From<ImportError> for AdapterError {
    fn from(e: ImportError) -> Self {
        Self::Other(e.to_string())
    }
}

/// A wanted-but-absent artefact, recorded in `acquisition_state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub kind: String,
    pub locator: String,
    /// Where a human should put the file to close this gap.
    pub drop_path: PathBuf,
    pub reason: String,
}

/// What one import run did, in a form the CLI can print and a test can
/// assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// The paper these artefacts belong to.
    pub paper_id: String,
    /// The paper directory that was walked, canonicalised.
    pub paper_dir: PathBuf,
    /// Artefact rows recorded for this work, by `kind`. Counts what the
    /// importer wrote, not what the work has accumulated over time.
    pub counts: BTreeMap<String, usize>,
    /// Distinct blobs copied into the store by this run.
    pub blobs: usize,
    /// Wanted-but-absent artefacts, each recorded as an
    /// `acquisition_state` row.
    pub gaps: Vec<Gap>,
    /// Paths inside the tree that resolved outside it and were refused.
    pub refused: Vec<PathBuf>,
    /// Files the `artefacts.kind` vocabulary has no place for. Left
    /// exactly where they were.
    pub unrecognised: Vec<PathBuf>,
}

impl ImportReport {
    /// How many rows of `kind` this run recorded.
    #[must_use]
    pub fn count(&self, kind: &str) -> usize {
        self.counts.get(kind).copied().unwrap_or(0)
    }

    /// Total artefact rows recorded.
    #[must_use]
    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }
}

/// Import a flat / legacy directory tree as `artefacts` rows for
/// `paper`.
///
/// `root` is the tree to import: either the paper's own directory, or a
/// library root containing `papers/<paper-stem>/` (the pre-0.9 layout).
/// Both are accepted, because "the directory raid wrote" and "the
/// library it wrote it into" are both things an operator will
/// reasonably have to hand. A directory that exists but matches neither
/// shape is imported as the paper directory itself, so an empty or
/// unrecognised directory reports an empty import rather than an error:
/// a directory with nothing in it is a fact about the library.
///
/// Idempotent: safe to run over the same tree any number of times. Never
/// moves or deletes anything in the tree.
///
/// # Errors
///
/// Only for problems with the *call* — no library root, no such
/// directory, or a SQLite failure. Per-file trouble lands in the report.
pub fn import_flat_tree(
    db: &Database,
    paper: &Paper,
    root: &Path,
) -> Result<ImportReport, ImportError> {
    let library_root = db.library_root()?.ok_or(ImportError::NoLibraryRoot)?;
    let paper_id = paper.id.as_str().to_string();
    let stem = file_stem_for(paper);

    let paper_dir = resolve_paper_dir(root, &stem)?;
    // One canonical root for the whole walk, so every containment check
    // compares like with like.
    let canonical_root = std::fs::canonicalize(&paper_dir).unwrap_or_else(|_| paper_dir.clone());

    let Scan {
        candidates,
        refused,
        unrecognised,
    } = scan_tree(&paper_dir, &canonical_root, &stem);
    let candidates = disambiguate(candidates);

    // Phase 1 — file work with no write lock held.
    let now = Utc::now();
    let rows: Vec<ArtefactWrite> = candidates
        .iter()
        .map(|candidate| plan(candidate, &paper_id, &library_root, now))
        .collect();
    let mut blob_digests = BTreeSet::new();
    blob_digests.extend(rows.iter().filter_map(|r| r.sha256.as_deref()));

    // Phase 2 — one short transaction for the rows.
    db.write_artefacts(&rows, WriteMode::Reconcile)?;

    let (holds_fulltext, mut gaps) = record_gaps(db, &paper_id, &candidates, &paper_dir, now)?;
    if holds_fulltext {
        // A gap this importer recorded is now closed; retract it rather
        // than leave "wanted" sitting next to "have".
        let drop_path = paper_dir
            .join(FULLTEXT_FILENAME)
            .to_string_lossy()
            .into_owned();
        match db.retract_acquisition_gap(&paper_id, &drop_path) {
            Ok(0) => {}
            Ok(n) => {
                tracing::debug!(paper_id = %paper_id, retracted = n, "closed a recorded gap");
                gaps.clear();
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not retract a closed acquisition gap");
            }
        }
    }

    let counts = rows.iter().fold(BTreeMap::new(), |mut acc, row| {
        *acc.entry(row.kind.clone()).or_insert(0) += 1;
        acc
    });

    Ok(ImportReport {
        paper_id,
        paper_dir: canonical_root,
        counts,
        blobs: blob_digests.len(),
        gaps,
        refused,
        unrecognised,
    })
}

/// Which slot of the flat layout a directory (or the paper directory
/// itself) is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    FullText,
    Si,
    Table,
    Figure,
}

impl Slot {
    /// The `artefacts.kind` this slot produces, or `None` for the
    /// full-text slot, whose kind comes from the file extension.
    const fn kind(self) -> Option<&'static str> {
        match self {
            Self::FullText => None,
            Self::Si => Some("si"),
            Self::Table => Some("table"),
            Self::Figure => Some("figure"),
        }
    }
}

/// One file found in the tree, before hashing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    /// Absolute, as walked. For a figure reference with no bytes this is
    /// the caption sidecar, which is where its information came from —
    /// and therefore what `imported_from` records.
    path: PathBuf,
    /// `false` only for a figure that exists solely as a caption
    /// (ADR-007 §1 "Artefact rules": `sha256 IS NULL` for exactly this
    /// case).
    has_bytes: bool,
    slot: Slot,
    /// The resolved `artefacts.kind`.
    kind: &'static str,
    /// The file's own stem — the human label (`Supplementary Table 1`).
    label: String,
    /// Normalised, stable locator (`supplementary-table-1`).
    locator: String,
    /// Lowercased extension; `None` for a caption-only reference.
    format: Option<String>,
    caption: Option<String>,
}

/// The result of walking the tree.
#[derive(Debug, Default)]
struct Scan {
    candidates: Vec<Candidate>,
    refused: Vec<PathBuf>,
    unrecognised: Vec<PathBuf>,
}

/// What a file name means in a given slot.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Classified {
    /// `kind` is already resolved: the full-text slot needs the
    /// extension table, the other slots map straight through.
    Artefact {
        slot: Slot,
        kind: &'static str,
        label: String,
        format: String,
    },
    /// Nothing outside the `kind` vocabulary; recorded in the report and
    /// left on disk.
    Unrecognised,
    /// Known, but not implemented in this slice (a `meta.json`
    /// manifest). Deliberately silent: importing half of one would
    /// claim more than we read.
    Deferred,
}

/// Find the paper's directory under `root`.
///
/// Three shapes, in order: the directory itself; `root/papers/<stem>`
/// (the pre-0.9 layout, and what `--root <library>` should resolve to);
/// `root/<stem>`. A directory that exists but matches none of them is
/// used as-is, so an empty tree imports as nothing instead of erroring.
fn resolve_paper_dir(root: &Path, stem: &str) -> Result<PathBuf, ImportError> {
    let no_such_dir = || ImportError::RootNotFound {
        root: root.to_path_buf(),
    };
    if !root.is_dir() {
        return Err(no_such_dir());
    }
    for candidate in [
        root.to_path_buf(),
        root.join("papers").join(stem),
        root.join(stem),
    ] {
        if looks_like_paper_dir(&candidate, stem) {
            return Ok(candidate);
        }
    }
    // Nothing matched, but the caller named a real directory: take it at
    // face value rather than refusing to report the emptiness.
    tracing::debug!(
        root = %root.display(),
        "no recognisable flat-layout files; importing the directory itself"
    );
    Ok(root.to_path_buf())
}

/// Does this directory hold anything the flat layout puts in it?
fn looks_like_paper_dir(dir: &Path, stem: &str) -> bool {
    if !dir.is_dir() {
        return false;
    }
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            let Some(name) = entry.file_name().to_str().map(str::to_ascii_lowercase) else {
                return false;
            };
            // A slot directory, a `fulltext.*` file, or the `<stem>.<ext>`
            // spelling `download_paper` writes.
            slot_for_dir(&name).is_some()
                || name.starts_with("fulltext.")
                || Path::new(&name)
                    .file_stem()
                    .is_some_and(|s| s.eq_ignore_ascii_case(stem))
        })
}

/// Walk `paper_dir`, classifying every file it finds.
///
/// `canonical_root` is the paper directory after symlink resolution;
/// anything whose canonical path is not inside it is refused without
/// being read.
fn scan_tree(paper_dir: &Path, canonical_root: &Path, stem: &str) -> Scan {
    let mut scan = Scan::default();
    let mut stack = vec![(paper_dir.to_path_buf(), Slot::FullText, 0_usize)];
    // (slot, label) → caption text, folded onto the file it describes
    // once the whole tree has been read.
    let mut captions: BTreeMap<(Slot, String), (String, PathBuf)> = BTreeMap::new();

    while let Some((dir, slot, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            tracing::warn!(
                dir = %dir.display(),
                max_depth = MAX_DEPTH,
                "flat-layout walk hit its depth cap; deeper files were not imported"
            );
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "could not read directory");
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `file_type` does not follow symlinks (unlike `metadata`),
            // so a symlinked directory is never descended into.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !within_root(canonical_root, &path) {
                tracing::warn!(
                    path = %path.display(),
                    root = %canonical_root.display(),
                    "refused an entry that resolves outside the imported tree"
                );
                scan.refused.push(path);
                continue;
            }
            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if let Some(child) = slot_for_dir(&name) {
                    stack.push((path, child, depth + 1));
                } else {
                    tracing::debug!(
                        path = %path.display(),
                        "not a flat-layout slot directory; its files were not imported"
                    );
                }
                continue;
            }

            let file_name = entry.file_name().to_string_lossy().into_owned();
            if let Some(caption_of) = caption_sidecar(slot, &file_name) {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        captions.insert(caption_of, (text.trim().to_string(), path));
                    }
                    Err(e) => tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "caption sidecar is not readable UTF-8; no caption recorded"
                    ),
                }
                continue;
            }
            match classify(slot, &file_name, stem) {
                Classified::Artefact {
                    slot,
                    kind,
                    label,
                    format,
                } => {
                    scan.candidates.push(Candidate {
                        path,
                        has_bytes: true,
                        slot,
                        kind,
                        locator: normalise_label(&label),
                        label,
                        format: Some(format),
                        caption: None,
                    });
                }
                Classified::Unrecognised => {
                    tracing::warn!(
                        path = %path.display(),
                        "file is not in the artefacts.kind vocabulary; no row recorded (file left in place)"
                    );
                    scan.unrecognised.push(path);
                }
                Classified::Deferred => tracing::debug!(
                    path = %path.display(),
                    "manifest file; parsing it is a later slice"
                ),
            }
        }
    }

    // Fold captions onto the files they describe.
    for candidate in &mut scan.candidates {
        if let Some((text, _)) = captions.remove(&(candidate.slot, candidate.label.clone())) {
            candidate.caption = Some(text);
        }
    }
    // A caption with no file is a figure reference: the digitisation
    // queue needs the URL and the caption, not the image.
    for ((slot, label), (text, path)) in captions {
        if slot != Slot::Figure {
            tracing::warn!(
                label = %label,
                "caption file has no matching artefact; nothing recorded (a NULL sha256 \
                 is only allowed for a figure reference)"
            );
            continue;
        }
        scan.candidates.push(Candidate {
            path,
            has_bytes: false,
            slot,
            kind: "figure",
            locator: normalise_label(&label),
            label,
            format: None,
            caption: Some(text),
        });
    }
    sort_candidates(&mut scan.candidates);
    scan
}

/// A deterministic order for the candidates: by slot, then locator,
/// then the already-canonical names first, then path.
///
/// The middle key matters: when `Table 1.csv` and `table-1.csv` both
/// normalise to `table-1`, the one that is already written the canonical
/// way keeps the bare locator, and the other is disambiguated. Sorting
/// by path alone would hand the bare locator to whichever name happens
/// to collate first.
fn sort_candidates(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        let key = |c: &Candidate| {
            (
                c.slot,
                c.locator.clone(),
                std::cmp::Reverse(c.label == c.locator),
                c.path.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
}

/// True when `path` — canonicalised — is `root` or lives under it.
///
/// Both sides must already be canonical: a path containing `..` is
/// resolved first, which is what stops `tables/../../secrets.csv` from
/// being read. `Path::starts_with` compares whole components, so
/// `/lib/papers/other` never passes for a child of `/lib/papers/this`.
fn within_root(root: &Path, path: &Path) -> bool {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| lexical_normalise(path));
    resolved == root || resolved.starts_with(root)
}

/// Resolve `.` and `..` in a path without touching the file system.
///
/// Canonicalisation is the primary defence, but it fails on a path that
/// does not exist — and a lexical comparison of a raw `..` path against
/// the root says "inside" when it means the opposite, which is exactly
/// the wrong answer for a security check. This is the fallback, so a
/// dangling symlink (which we *do* want to keep, since it can only point
/// at bytes we would then fail to read) still passes while a `..` escape
/// does not.
fn lexical_normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // Never climb above a rooted path's root, and never
                // discard an unresolved leading `..`.
                if out
                    .components()
                    .next_back()
                    .is_some_and(|c| matches!(c, std::path::Component::Normal(_)))
                {
                    out.pop();
                } else if path.is_relative() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// What a file name means in a given slot.
fn classify(slot: Slot, file_name: &str, stem: &str) -> Classified {
    let ext = file_extension(file_name);
    let lower = file_name.to_ascii_lowercase();
    let label = Path::new(file_name)
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());

    if is_manifest(&lower) {
        // ADR-007 §1 "Legacy data" also lists `meta.json` /
        // `tables.json` manifests; parsing those is a later slice.
        return Classified::Deferred;
    }
    if ext.is_empty() || label.is_empty() {
        return Classified::Unrecognised;
    }

    match slot {
        Slot::FullText => {
            // The full-text slot decides by extension, via the same
            // table the legacy backfill uses.
            if let Some(kind) = fulltext_kind(&ext).filter(|_| is_fulltext_name(&lower, stem)) {
                return Classified::Artefact {
                    slot: Slot::FullText,
                    kind: kind.kind,
                    label,
                    format: ext,
                };
            }
            // raid's single-directory spelling: `<slug>_SI_<name>.<ext>`.
            if let Some(name) = split_supplement_marker(&lower) {
                return artefact(Slot::Si, name, ext);
            }
            Classified::Unrecognised
        }
        // SI is unbounded: a supplementary file can be anything.
        Slot::Si => artefact(Slot::Si, label, ext),
        Slot::Table if is_table_format(&ext) => artefact(Slot::Table, label, ext),
        Slot::Figure if is_figure_format(&ext) => artefact(Slot::Figure, label, ext),
        _ => Classified::Unrecognised,
    }
}

/// A `Classified::Artefact` for a slot whose `kind` is fixed by the slot
/// itself. The full-text slot is never routed through here — its kind
/// comes from the extension table.
fn artefact(slot: Slot, label: String, format: String) -> Classified {
    Classified::Artefact {
        slot,
        kind: slot
            .kind()
            .expect("the full-text slot resolves its kind from the extension"),
        label,
        format,
    }
}

/// `true` for a full-text filename in the flat layout: the fixed
/// `fulltext.<ext>`, or the `<stem>.<ext>` spelling `download_paper`
/// writes and `find_cached_file` looks for.
fn is_fulltext_name(lower_name: &str, stem: &str) -> bool {
    if lower_name.starts_with("fulltext.") {
        return true;
    }
    let suffix = lower_name
        .strip_prefix(&stem.to_ascii_lowercase())
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or_default();
    matches!(
        suffix,
        "pdf" | "html" | "htm" | "xhtml" | "xml" | "nxml" | "jats"
    )
}

/// raid writes supplements as `<slug>_SI_<name>.<ext>`; the label is the
/// part after the marker.
fn split_supplement_marker(lower_name: &str) -> Option<String> {
    let at = lower_name.find("_si_")?;
    let rest = &lower_name[at + 4..];
    let label = rest.rsplit_once('.').map_or(rest, |(head, _)| head);
    (!label.is_empty()).then(|| label.to_string())
}

fn is_manifest(lower_name: &str) -> bool {
    matches!(
        lower_name,
        "meta.json" | "tables.json" | "manifest.json" | "figures.json"
    )
}

/// The slot a subdirectory name implies. Case-insensitive, so a tree
/// written `Tables/` is recognised as readily as `tables/`.
fn slot_for_dir(name: &str) -> Option<Slot> {
    match name.to_ascii_lowercase().as_str() {
        "si" | "supplement" | "supplementary" | "supplements" => Some(Slot::Si),
        "tables" | "table" => Some(Slot::Table),
        "figures" | "figure" | "figs" => Some(Slot::Figure),
        _ => None,
    }
}

/// Table serialisations worth a `table` row. ADR-007 §1 "Artefact
/// rules" notes the structured form is JSON and CSV cannot express
/// merged cells, so both are accepted and the format is recorded either
/// way.
fn is_table_format(ext: &str) -> bool {
    matches!(ext, "csv" | "tsv" | "json" | "xlsx" | "xls" | "ods")
}

/// Image formats worth a `figure` row.
fn is_figure_format(ext: &str) -> bool {
    matches!(
        ext,
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "svg"
            | "webp"
            | "tif"
            | "tiff"
            | "bmp"
            | "avif"
            | "eps"
            | "pdf"
    )
}

/// The slot and label a `<label>.caption.txt` sidecar belongs to, if it
/// is one. Captions only accompany artefacts that carry a label.
fn caption_sidecar(slot: Slot, file_name: &str) -> Option<(Slot, String)> {
    if !matches!(slot, Slot::Figure | Slot::Table) {
        return None;
    }
    let label = file_name
        .strip_suffix(".caption.txt")
        .or_else(|| file_name.strip_suffix(".CAPTION.TXT"))?;
    (!label.is_empty()).then(|| (slot, label.to_string()))
}

/// Normalise a label into a stable locator: lowercase, every run of
/// non-alphanumeric characters collapsed to `-`, no leading or trailing
/// `-`. `Supplementary Table 1` → `supplementary-table-1`, `table-1` →
/// `table-1`.
///
/// Never empty: a name made only of punctuation falls back to a
/// punctuation-preserving form, which is still stable.
#[must_use]
fn normalise_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_separator = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push('-');
            }
            pending_separator = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_separator = true;
        }
    }
    if out.is_empty() {
        raw.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    } else {
        out
    }
}

/// Make every `(kind, locator)` unique within one run.
///
/// Two files normalising to the same label (`Table 1.csv` and
/// `Table 1.json`, or `fig1.png` and `fig1.jpg`) would collide on
/// `UNIQUE (paper_id, kind, version, locator)`, and the second would
/// silently overwrite the first — a lossy import. Instead the first
/// keeps the bare locator and the rest take the extension, then an
/// index. Candidates are walked in sorted order, so the same tree always
/// produces the same locators.
fn disambiguate(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut candidates = candidates;
    let mut taken: BTreeSet<(Slot, String)> = candidates
        .iter()
        .map(|c| (c.slot, c.locator.clone()))
        .collect();
    let mut seen: BTreeSet<(Slot, String)> = BTreeSet::new();
    for candidate in &mut candidates {
        let key = (candidate.slot, candidate.locator.clone());
        if seen.insert(key) {
            continue;
        }
        // Second and later arrivals: qualify with the format, then with
        // an index if that is still taken.
        let qualified = match (&candidate.format, candidate.has_bytes) {
            (Some(format), true) => format!("{}.{format}", candidate.locator),
            _ => format!("{}.caption", candidate.locator),
        };
        let mut unique = qualified.clone();
        let mut n = 2;
        while !taken.insert((candidate.slot, unique.clone())) {
            unique = format!("{qualified}-{n}");
            n += 1;
        }
        tracing::warn!(
            locator = %candidate.locator,
            resolved_to = %unique,
            path = %candidate.path.display(),
            "two files normalised to one locator; disambiguated rather than dropped"
        );
        candidate.locator = unique;
    }
    candidates
}

/// Phase 1 for one candidate: hash the file, copy it into the blob
/// store, and build the row.
fn plan(
    candidate: &Candidate,
    paper_id: &str,
    library_root: &Path,
    now: DateTime<Utc>,
) -> ArtefactWrite {
    let full_text = candidate.slot == Slot::FullText;
    let mut row = ArtefactWrite {
        id: String::new(),
        paper_id: paper_id.to_string(),
        kind: candidate.kind.to_string(),
        version: VERSION_UNKNOWN.to_string(),
        locator: if full_text {
            FULLTEXT_LOCATOR.to_string()
        } else {
            candidate.locator.clone()
        },
        sha256: None,
        format: candidate.format.clone(),
        // Full-text bytes are the full text; no machine classified a
        // supplementary file, a table or a figure, and the column has
        // no better value than `unknown`.
        access_status: if full_text { "full_text" } else { "unknown" }.to_string(),
        route: ROUTE_IMPORT_FLAT.to_string(),
        access_basis: ACCESS_BASIS_MANUAL.to_string(),
        label: (!full_text).then(|| candidate.label.clone()),
        caption: candidate.caption.clone(),
        source_url: None,
        imported_from: Some(candidate.path.to_string_lossy().into_owned()),
        retrieved_at: modified_at(&candidate.path).unwrap_or(now).to_rfc3339(),
        missing_on_disk: false,
        blob: None,
    };

    if !candidate.has_bytes {
        // A figure reference with no bytes: `sha256` stays NULL and
        // `missing_on_disk` stays 0, because nothing is missing — there
        // are simply no image bytes to hold.
        return row;
    }

    let ext = candidate.format.clone().unwrap_or_default();
    let (sha256, bytes) = match hash_file(&candidate.path) {
        Ok(hashed) => hashed,
        Err(e) => {
            // Gone or unreadable. The row stays, naming where the file
            // was, with no hash — ADR-007's own rule for a recorded path
            // with no usable bytes.
            tracing::warn!(
                path = %candidate.path.display(),
                error = %e,
                "imported file is not readable; recorded as missing_on_disk"
            );
            row.missing_on_disk = true;
            return row;
        }
    };
    row.sha256 = Some(sha256.clone());
    row.blob = Some(BlobWrite {
        rel_path: blob_rel_path(&sha256, &ext),
        sha256: sha256.clone(),
        bytes,
        mime: mime_for(&ext).to_string(),
        created_at: now.to_rfc3339(),
    });
    // A blob we identified but could not place is a fact to flag, not a
    // reason to lose the row: every later run retries the copy, and the
    // file itself is untouched.
    if let Err(e) = store_blob(&candidate.path, &sha256, &ext, library_root) {
        tracing::warn!(
            path = %candidate.path.display(),
            error = %e,
            "hashed an imported file but could not copy it into the blob store"
        );
        row.missing_on_disk = true;
    }
    row
}

/// The file's own mtime as RFC 3339 UTC, or `None` when the platform
/// cannot say. `retrieved_at` is *when the content was retrieved*, so it
/// has to come from the file: stamping it with "now" would make every
/// re-import of an unchanged tree a change.
fn modified_at(path: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()
        .map(DateTime::<Utc>::from)
}

/// Record what the layout wanted and did not contain.
///
/// Returns whether the work holds a full text (from this tree or
/// already in the database) alongside the gaps recorded. Only the full
/// text is checked: the flat layout has a slot for it, so its absence is
/// evidence, and `coverage` can then report the gap instead of the work
/// looking simply un-acquired. Whether a work *has* SI, tables or figures
/// is not something a directory can tell us, so no gap is invented.
fn record_gaps(
    db: &Database,
    paper_id: &str,
    candidates: &[Candidate],
    paper_dir: &Path,
    now: DateTime<Utc>,
) -> Result<(bool, Vec<Gap>), ImportError> {
    let from_tree = candidates.iter().any(|c| c.slot == Slot::FullText);
    let holds_fulltext = from_tree || db.has_fulltext_artefact(paper_id)?;
    if holds_fulltext {
        return Ok((true, Vec::new()));
    }

    let drop_path = paper_dir.join(FULLTEXT_FILENAME);
    let gap = Gap {
        kind: "fulltext".to_string(),
        locator: FULLTEXT_LOCATOR.to_string(),
        drop_path: drop_path.clone(),
        reason: format!(
            "no fulltext.pdf / fulltext.html in the imported flat tree {}",
            paper_dir.display()
        ),
    };
    db.upsert_acquisition_state(&StateWrite {
        paper_id: paper_id.to_string(),
        kind: gap.kind.clone(),
        locator: gap.locator.clone(),
        wanted_version: "vor".to_string(),
        status: "pending".to_string(),
        reason: Some(gap.reason.clone()),
        hint_url: None,
        drop_path: Some(drop_path.to_string_lossy().into_owned()),
        updated_at: now.to_rfc3339(),
    })?;
    Ok((false, vec![gap]))
}

/// MIME type for an imported file's extension. SI is unbounded, so this
/// falls back to `application/octet-stream` rather than refusing.
fn mime_for(ext: &str) -> &'static str {
    match ext {
        "pdf" => "application/pdf",
        "html" | "htm" | "xhtml" => "text/html",
        "xml" | "nxml" | "jats" => "application/xml",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "txt" => "text/plain",
        "zip" => "application/zip",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "tif" | "tiff" => "image/tiff",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "eps" => "application/postscript",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests;
