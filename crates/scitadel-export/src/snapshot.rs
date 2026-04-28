//! Pure write-side of `bib snapshot` (#178), reusable from CLI + TUI.
//!
//! The CLI command in `scitadel-cli` already loads a question's
//! shortlist + papers + tags from the DB, then renders + writes the
//! `.bib` (or CSL-JSON) file plus the `.scitadel-bib.lock` sidecar.
//! The TUI needs to do the exact same write step from already-loaded
//! data when the user presses `E` on the Question Dashboard
//! (#135 sub-feature B).
//!
//! Lifting the rendering + I/O into this module guarantees both
//! surfaces produce byte-identical artifacts — there is no second copy
//! of the format-routing or sidecar-construction logic.

use std::path::{Path, PathBuf};

use scitadel_core::models::Paper;

use crate::sidecar::BibLockfile;
use crate::{export_bibtex_with_tags, export_csl_json_with_tags};

/// Sidecar suffix appended to the snapshot's output path. One sidecar
/// per `(question, output_file)` keeps scope focused — see #178
/// "Sidecar location collision" pitfall.
pub const SIDECAR_SUFFIX: &str = ".scitadel-bib.lock";

/// Output flavor for [`write_snapshot`]. The string discriminants live
/// on [`crate::sidecar`] so verify and snapshot can never disagree
/// about the wire value of the format field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFormat {
    BibTeX,
    CslJson,
}

impl SnapshotFormat {
    /// Suggested file extension, including the leading dot. Used by
    /// callers building default output paths.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::BibTeX => ".bib",
            Self::CslJson => ".json",
        }
    }
}

/// Default filename (path component, no directory) for a given
/// snapshot format. Mirrors the CLI's `--output` fallback so `E` from
/// the TUI without typing anything produces the same filename you'd
/// get from `bib snapshot <id>` with no `--output`.
#[must_use]
pub fn default_filename_for_format(format: SnapshotFormat) -> &'static Path {
    match format {
        SnapshotFormat::CslJson => Path::new("paper.json"),
        SnapshotFormat::BibTeX => Path::new("paper.bib"),
    }
}

/// Path of the `.scitadel-bib.lock` sidecar that should sit next to
/// `bib`. The sidecar's filename is exactly `<bib>.scitadel-bib.lock`
/// — see `bib snapshot` docs.
#[must_use]
pub fn sidecar_path_for(bib: &Path) -> PathBuf {
    let mut s = bib.as_os_str().to_owned();
    s.push(SIDECAR_SUFFIX);
    PathBuf::from(s)
}

/// What [`write_snapshot`] just wrote to disk. The TUI surfaces the
/// counts in a status-bar toast; tests assert the on-disk artifacts
/// against the returned paths.
#[derive(Debug, Clone)]
pub struct SnapshotOutcome {
    /// Path the rendered bibliography was written to (verbatim from the
    /// caller; sidecar is computed from this).
    pub output_path: PathBuf,
    /// Path of the `.scitadel-bib.lock` sidecar; `None` when the caller
    /// asked us to skip the sidecar via `write_lockfile = false`.
    pub sidecar_path: Option<PathBuf>,
    /// Number of `Paper` rows the bibliography includes. Drives the
    /// "exported: paper.bib (N entries)" toast in the TUI.
    pub entry_count: usize,
}

/// Write a snapshot to disk from already-loaded inputs.
///
/// `papers` and `paper_ids` are expected to be in shortlist order.
/// `tags_for(paper_id)` returns the per-paper BibTeX tags (groups);
/// callers without tags pass a closure that always returns `vec![]`.
///
/// # Errors
///
/// Returns any [`std::io::Error`] from writing the bibliography or the
/// sidecar. Lockfile JSON serialization errors are wrapped in
/// `io::ErrorKind::Other`.
#[allow(clippy::too_many_arguments)]
pub fn write_snapshot<F>(
    output_path: &Path,
    question_id: &str,
    reader: &str,
    papers: &[Paper],
    paper_ids: &[String],
    tags_for: F,
    format: SnapshotFormat,
    write_lockfile: bool,
) -> std::io::Result<SnapshotOutcome>
where
    F: Fn(&str) -> Vec<String>,
{
    let (content, lock) = match format {
        SnapshotFormat::BibTeX => {
            let c = export_bibtex_with_tags(papers, &tags_for);
            let l = BibLockfile::new_bibtex(question_id, reader, paper_ids, &c);
            (c, l)
        }
        SnapshotFormat::CslJson => {
            let c = export_csl_json_with_tags(papers, &tags_for);
            let l = BibLockfile::new_csl_json(question_id, reader, paper_ids, &c);
            (c, l)
        }
    };

    std::fs::write(output_path, &content)?;

    let sidecar_path = if write_lockfile {
        let path = sidecar_path_for(output_path);
        let json = lock.to_json().map_err(std::io::Error::other)?;
        std::fs::write(&path, json)?;
        Some(path)
    } else {
        None
    };

    Ok(SnapshotOutcome {
        output_path: output_path.to_path_buf(),
        sidecar_path,
        entry_count: papers.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::{Paper, PaperId};

    fn paper(id: &str, title: &str, year: i32) -> Paper {
        let mut p = Paper::new(title);
        p.id = PaperId::from(id);
        p.authors = vec!["Doe, J.".into()];
        p.year = Some(year);
        p
    }

    #[test]
    fn sidecar_path_appends_suffix() {
        let p = sidecar_path_for(Path::new("out/paper.bib"));
        assert_eq!(p, PathBuf::from("out/paper.bib.scitadel-bib.lock"));
    }

    #[test]
    fn default_filename_matches_format() {
        assert_eq!(
            default_filename_for_format(SnapshotFormat::BibTeX),
            Path::new("paper.bib")
        );
        assert_eq!(
            default_filename_for_format(SnapshotFormat::CslJson),
            Path::new("paper.json")
        );
    }

    #[test]
    fn write_snapshot_bibtex_writes_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("p.bib");
        let papers = vec![paper("p-1", "On Attention", 2017)];
        let ids = vec!["p-1".to_string()];

        let outcome = write_snapshot(
            &out,
            "q-x",
            "lars",
            &papers,
            &ids,
            |_| vec![],
            SnapshotFormat::BibTeX,
            true,
        )
        .unwrap();

        assert_eq!(outcome.entry_count, 1);
        assert_eq!(outcome.output_path, out);
        let sidecar = outcome.sidecar_path.unwrap();
        assert_eq!(sidecar, out.with_extension("bib.scitadel-bib.lock"));
        let bib_bytes = std::fs::read_to_string(&out).unwrap();
        assert!(bib_bytes.contains("On Attention"), "got: {bib_bytes}");
        let lock_bytes = std::fs::read_to_string(&sidecar).unwrap();
        let lock: BibLockfile = serde_json::from_str(&lock_bytes).unwrap();
        assert_eq!(lock.question_id, "q-x");
        assert_eq!(lock.format, "bibtex");
    }

    #[test]
    fn write_snapshot_csl_json_writes_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("p.json");
        let papers = vec![paper("p-1", "On Attention", 2017)];
        let ids = vec!["p-1".to_string()];

        let outcome = write_snapshot(
            &out,
            "q-x",
            "lars",
            &papers,
            &ids,
            |_| vec![],
            SnapshotFormat::CslJson,
            true,
        )
        .unwrap();

        let lock: BibLockfile =
            serde_json::from_str(&std::fs::read_to_string(outcome.sidecar_path.unwrap()).unwrap())
                .unwrap();
        assert_eq!(lock.format, "csl-json");
    }

    #[test]
    fn write_snapshot_skips_sidecar_when_requested() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("p.bib");
        let papers = vec![paper("p-1", "T", 2020)];
        let ids = vec!["p-1".to_string()];

        let outcome = write_snapshot(
            &out,
            "q",
            "r",
            &papers,
            &ids,
            |_| vec![],
            SnapshotFormat::BibTeX,
            false,
        )
        .unwrap();

        assert!(outcome.sidecar_path.is_none());
        assert!(out.exists());
    }
}
