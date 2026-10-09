//! ADR-007 §1: `papers/<stem>/manifest.json` — the **generated, read-only
//! mirror**.
//!
//! > The database is the source of truth, and `papers/<stem>/manifest.json` is
//! > a generated, read-only mirror.
//! >
//! > **The manifest mirror** is written only by the lease holder, after the DB
//! > commit, via a temp file and a rename. It is never read as input.
//!
//! Four constraints, and each of them is load-bearing enough that this module
//! exists mostly to hold them.
//!
//! ## It is never read as input
//!
//! Nothing in the workspace parses this file — not `scan`, not `attach`, not
//! `acquire`, not `coverage`, not the TUI. There is exactly one function in this
//! module that touches the path, [`write`], and it only writes. That is the
//! whole mechanism: a mirror that is read back becomes a second source of truth
//! and the two drift, and the drift is invisible because the file *looks*
//! authoritative. `the_manifest_is_never_read_as_input` pins it by poisoning a
//! manifest with a claim the database does not make and showing that every
//! derived answer is unchanged.
//!
//! ## It is written after the DB commit
//!
//! [`write`] is not part of any transaction, and every caller reaches it only
//! after the artefact rows have committed. So a manifest describes a state the
//! database was actually in; a manifest can never describe a state that was
//! rolled back. The alternative — writing it inside the transaction — would put
//! a filesystem write under the write lock (a slow thing to do while every other
//! process waits) and would produce a file describing rows that a later
//! rollback erases.
//!
//! Its one consequence is worth stating: **the mirror can lag.** A crash between
//! the commit and the write leaves the database right and the file stale, and the
//! next lease-holder run repairs it. That is the correct way round — a stale
//! mirror is regenerated, whereas a missing row is a lost file.
//!
//! ## It is written only by the lease holder
//!
//! [`write`] refuses unless `owner` currently holds the work's lease
//! ([`scitadel_db::sqlite::leases`]). That is not politeness about who may write
//! where: two processes writing `manifest.json` through a temp file would
//! produce a file that is a valid-looking interleaving of neither, and the
//! manifest is a published provenance document.
//!
//! ## Via a temp file and a rename
//!
//! [`write_atomically`] stages in the *destination directory* — same filesystem,
//! which is what makes `rename` atomic — and renames over the destination. A
//! reader therefore sees either the old manifest or the new one, never a
//! half-written one, and a crash mid-write leaves the previous manifest intact.
//! The staging name carries the pid, matching `blobs.rs`'s discipline, so two
//! processes in the same library never share a staging path.
//!
//! ## What is in it
//!
//! The shape `issue-234` specified: `work` (identity plus the latest identity
//! check), `artefacts` (one entry per row, with the **root-relative** blob path
//! — ADR-007 §1 "Storage": absolute paths break as soon as a library moves) and
//! `missing` (the same derivation `coverage` reports).
//!
//! Artefacts are sorted and the document serialised deterministically, so an
//! unchanged work produces the same bytes **except** for `generated_at`, which is
//! the clock at the moment of generation — so a lease holder's run always rewrites
//! the file. That is deliberate and it is the one place where "idempotent" does not
//! mean "no write": the *rows* are unchanged, which is the claim `scan` makes and
//! the one a reader can check, while a mirror whose timestamp moves is honest
//! about when it was produced rather than pretending to be a checksum.
//!
//! ## A field nothing recorded is omitted, not emitted as `null` (#291)
//!
//! The four `license_*` fields are the case that forced the rule: for most of
//! this project's life nothing could write them, so every artefact's mirror
//! carried `"license_url": null` — which reads as "we looked and there is no
//! licence", the exact assertion-of-absence #291 exists to stop. A field with
//! no value is now left out of the JSON, and only [`ManifestArtefact`]'s four
//! licence fields are affected, because they are the ones whose absence is a
//! claim about the world. A reader that needs to know whether a licence was
//! recorded asks whether the key is present, which is the same question the
//! column answers.

use std::path::{Path, PathBuf};

use scitadel_core::models::Paper;
use scitadel_db::error::DbError;
use scitadel_db::sqlite::{Database, IdentityPhase};
use serde::{Deserialize, Serialize};

use crate::download::file_stem_for;

/// The mirror's file name inside a work's directory.
///
/// Named here rather than spelled at call sites, because the "never read as
/// input" guarantee is only true if there is exactly one spelling of the path in
/// the workspace to be wrong about.
pub const MANIFEST_FILENAME: &str = "manifest.json";

/// The directory the mirrors live in, relative to the library root: ADR-007's
/// `papers/<stem>/manifest.json`, and the same directory
/// `scitadel_core::config::Config::papers_dir` names for the legacy copies.
pub const MANIFEST_DIR: &str = "papers";

/// The mirror for `paper`: `<library_root>/papers/<stem>/manifest.json`.
#[must_use]
pub fn manifest_path(library_root: &Path, paper: &Paper) -> PathBuf {
    library_root
        .join(MANIFEST_DIR)
        .join(file_stem_for(paper))
        .join(MANIFEST_FILENAME)
}

/// The generated mirror document.
///
/// `Deserialize` is derived for **tests and for a reader who wants to inspect a
/// published file**; no library code path constructs one from a manifest, and
/// [`write`] is the only function in this module that touches the path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// ADR-007 §1's mirror shape. `stem` is here so a reader of the file can
    /// tell which work directory it belongs to without walking up.
    pub work: ManifestWork,
    pub artefacts: Vec<ManifestArtefact>,
    /// What the ADR-007 §1 derivation says we want and do not hold. The same
    /// computation `coverage` reports, projected onto one work.
    pub missing: Vec<ManifestMissing>,
    /// When the mirror was generated. The one field that legitimately changes
    /// on every write, and the reason the file is a mirror rather than a
    /// checksum.
    pub generated_at: String,
}

/// The work, as the mirror records it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestWork {
    pub paper_id: String,
    pub stem: String,
    pub doi: Option<String>,
    pub pmcid: Option<String>,
    pub osti_id: Option<String>,
    pub arxiv_id: Option<String>,
    pub openalex_id: Option<String>,
    pub title: String,
    pub year: Option<i32>,
    pub first_author: Option<String>,
    /// The ADR-007 §3 check in force after a fetch, for both phases. `None`
    /// when no check has been taken, which is **not** the same as a pass.
    pub identity_check: Option<ManifestIdentityCheck>,
}

/// One identity check, so a reader of the mirror can see both titles of a
/// dispute without opening the database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestIdentityCheck {
    pub phase: String,
    pub source: String,
    pub expected_title: Option<String>,
    pub resolved_title: Option<String>,
    pub score: Option<f64>,
    pub status: String,
    pub override_reason: Option<String>,
}

/// One artefact row, in the mirror.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestArtefact {
    pub id: String,
    pub kind: String,
    pub format: Option<String>,
    pub version: String,
    pub locator: String,
    /// Root-relative blob path, or `None` for a figure reference with no bytes.
    pub path: Option<String>,
    pub sha256: Option<String>,
    pub bytes: Option<i64>,
    pub route: String,
    pub access_status: String,
    pub access_basis: String,
    pub publisher: Option<String>,
    /// The licence, as migration 013's four columns carry it. **Omitted from
    /// the JSON when nothing was recorded**, and that is the whole of #291's
    /// second proposal: a mirror that emits `"license_url": null` reads as "we
    /// looked and there is no licence", which is a claim about the world that
    /// no writer has made. An absent key reads as "not recorded", which is
    /// what the column actually means.
    ///
    /// Omission rather than a note because the field's *type* has to stay
    /// stable: a consumer deciding whether derived data may redistribute reads
    /// `license_url` as a string, and an object that says `{"recorded": false}`
    /// would make every reader handle two shapes for one question. `Option` is
    /// already the field's own word for "absent", so the mirror says it that
    /// way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_content_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_source: Option<String>,
    pub label: Option<String>,
    pub caption: Option<String>,
    pub source_url: Option<String>,
    /// ADR-007 §1 "Legacy data": the original absolute path, for a published
    /// datasheet that cites it as provenance. The only absolute path in the
    /// document, and it is provenance rather than a place to read from.
    pub imported_from: Option<String>,
    pub retrieved_at: String,
    pub missing_on_disk: bool,
}

/// One wanted-but-not-held artefact, in the mirror.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestMissing {
    pub kind: String,
    pub locator: String,
    pub wanted_version: String,
    pub status: String,
    pub reason: Option<String>,
    pub publisher: Option<String>,
    pub hint_url: Option<String>,
    pub drop_path: Option<String>,
    pub next_attempt_at: Option<String>,
}

impl From<scitadel_db::sqlite::ArtefactRow> for ManifestArtefact {
    /// A projection, not a re-read: every column the row has is carried, so the
    /// mirror cannot be a lossy copy of what the database says.
    fn from(row: scitadel_db::sqlite::ArtefactRow) -> Self {
        Self {
            id: row.id,
            kind: row.kind,
            format: row.format,
            version: row.version,
            locator: row.locator,
            path: row.blob_rel_path,
            sha256: row.sha256,
            bytes: row.bytes,
            route: row.route,
            access_status: row.access_status,
            access_basis: row.access_basis,
            publisher: row.publisher,
            license_url: row.license_url,
            license_content_version: row.license_content_version,
            license_start: row.license_start,
            license_source: row.license_source,
            label: row.label,
            caption: row.caption,
            source_url: row.source_url,
            imported_from: row.imported_from,
            retrieved_at: row.retrieved_at,
            missing_on_disk: row.missing_on_disk,
        }
    }
}

/// What one write did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestWrite {
    /// Where the mirror was written, or would be.
    pub path: PathBuf,
    /// `true` when bytes were actually placed. `false` for a dry run.
    pub written: bool,
    /// The lease the writer held, echoed so a report can name who regenerated
    /// the mirror.
    pub owner: String,
}

/// Anything that stops the mirror being written.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// ADR-007 §1: "written only by the lease holder". The caller did not hold
    /// the work's lease, so it is not allowed to write the mirror.
    /// ADR-007 §1: "written only by the lease holder". A pre-built message,
    /// because the useful part of it differs — somebody else holds the work, or
    /// nobody does — and a template with a conditional suffix reads as one
    /// sentence with a hole in it.
    #[error("{0}")]
    NotLeaseHolder(String),
    #[error(
        "the manifest mirror needs a file-backed library: an in-memory database has no root \
         (ADR-007 §1 \"Storage\")"
    )]
    NoLibraryRoot,
    #[error(transparent)]
    Db(#[from] DbError),
    /// The ADR-007 §1 "Have" derivation refused to run — an unknown want kind,
    /// which cannot happen through the CLI's parser but can through a caller.
    #[error(transparent)]
    Coverage(#[from] scitadel_db::sqlite::CoverageError),
    #[error("could not write the manifest mirror at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Build the mirror for `paper` from the database alone.
///
/// The signature is the module's argument in one line: every input is a read of
/// `db`, and there is no path parameter, so there is nothing for a caller to
/// pass in that could come from a manifest.
pub fn build(db: &Database, paper: &Paper) -> Result<Manifest, ManifestError> {
    let paper_id = paper.id.as_str();
    let work = ManifestWork {
        paper_id: paper_id.to_string(),
        stem: file_stem_for(paper),
        doi: paper.doi.clone(),
        pmcid: db.pmcid(paper_id)?,
        osti_id: db.osti_id(paper_id)?,
        arxiv_id: paper.arxiv_id.clone(),
        openalex_id: paper.openalex_id.clone(),
        title: paper.title.clone(),
        year: paper.year,
        first_author: paper.authors.first().cloned(),
        // The post-fetch check is the one that can say `mismatch`, so it leads;
        // a work with only a pre-fetch check still shows that one.
        identity_check: [IdentityPhase::PostFetch, IdentityPhase::PreFetch]
            .into_iter()
            .find_map(|phase| {
                db.latest_identity_check(paper_id, phase)
                    .ok()
                    .flatten()
                    .map(|check| ManifestIdentityCheck {
                        phase: check.phase.label().to_string(),
                        source: check.source.label().to_string(),
                        expected_title: check.expected_title,
                        resolved_title: check.resolved_title,
                        score: check.score,
                        status: check.status.label().to_string(),
                        override_reason: check.override_reason,
                    })
            }),
    };

    let artefacts = db
        .artefacts_for_paper(paper_id)?
        .into_iter()
        .map(ManifestArtefact::from)
        .collect();
    let report = db.coverage_report(None)?;
    let missing = report
        .entries
        .iter()
        .filter(|entry| entry.paper_id == paper_id)
        .map(|entry| ManifestMissing {
            kind: entry.kind.clone(),
            locator: entry.locator.clone(),
            wanted_version: entry.wanted_version.clone(),
            status: entry.status.clone(),
            reason: entry.reason.clone(),
            publisher: entry.publisher.clone(),
            hint_url: entry.hint_url.clone(),
            drop_path: entry.drop_path.clone(),
            next_attempt_at: db
                .acquisition_state(paper_id, &entry.kind, &entry.locator)
                .ok()
                .flatten()
                .and_then(|row| row.next_attempt_at),
        })
        .collect();

    Ok(Manifest {
        work,
        artefacts,
        missing,
        generated_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Write the mirror for `paper`, **as the work's lease holder**.
///
/// Refuses with [`ManifestError::NotLeaseHolder`] for anybody else, and never
/// reads an existing mirror: the document is rebuilt from the database every
/// time, so a stale or hand-edited file cannot influence what is written.
pub fn write(db: &Database, paper: &Paper, owner: &str) -> Result<ManifestWrite, ManifestError> {
    let paper_id = paper.id.as_str();
    let held = db.lease_holder(paper_id)?;
    if held.as_ref().map(|l| l.owner.as_str()) != Some(owner) {
        let who = match &held {
            Some(lease) => format!(
                "{} holds it until lease_until_ms {}",
                lease.owner, lease.lease_until_ms
            ),
            None => {
                String::from("nobody holds it — claim the work with `acquisition_leases` first")
            }
        };
        return Err(ManifestError::NotLeaseHolder(format!(
            "the manifest for {paper_id} is written only by the work's lease holder, and {owner:?} \
             does not hold it ({who})"
        )));
    }
    let library_root = db.library_root()?.ok_or(ManifestError::NoLibraryRoot)?;
    let manifest = build(db, paper)?;
    let path = manifest_path(&library_root, paper);
    let body = serde_json::to_string_pretty(&manifest).map_err(|source| ManifestError::Io {
        path: path.clone(),
        source: std::io::Error::other(source),
    })?;
    write_atomically(&path, body.as_bytes())?;
    Ok(ManifestWrite {
        path,
        written: true,
        owner: owner.to_string(),
    })
}

/// The staging name for `path`, next to it on the same filesystem.
///
/// Public to the module's own tests rather than to callers: the invariant worth
/// asserting (same directory, pid in the name) is a property of this function,
/// not something a caller may rely on.
fn staging_path(path: &Path) -> PathBuf {
    let name = path.file_name().map_or_else(
        || MANIFEST_FILENAME.to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!("{name}.{}.tmp", std::process::id()))
}

/// Write `bytes` to `path` via a staged file and a rename.
///
/// Three properties, each of which is the reason for the mechanism rather than a
/// nicety of it:
///
/// - **same directory, so the rename is atomic** — a `rename` across
///   filesystems is a copy, and a reader would see a truncated file;
/// - **staged name carries the pid** — two processes regenerating different
///   works under one library root cannot share a staging path, so a crash cannot
///   leave one process's half-written bytes under another's name;
/// - **the old file survives a failure** — the destination is only ever replaced
///   by a complete `rename`, and a staging failure removes only the staging file.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), ManifestError> {
    let dir = path.parent().ok_or_else(|| ManifestError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other("the manifest path has no parent directory"),
    })?;
    std::fs::create_dir_all(dir).map_err(|source| ManifestError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    let staging = staging_path(path);
    // A leftover staging file from a crashed process must never be mistaken for
    // a manifest, and must never be reused.
    let _ = std::fs::remove_file(&staging);
    std::fs::write(&staging, bytes).map_err(|source| ManifestError::Io {
        path: staging.clone(),
        source,
    })?;

    match std::fs::rename(&staging, path) {
        Ok(()) => Ok(()),
        Err(source) => {
            // The previous mirror is untouched, which is the whole point of
            // staging; report where the staging file is so it can be removed.
            let _ = std::fs::remove_file(&staging);
            Err(ManifestError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    //! The four ADR-007 constraints, each pinned by the failure it prevents.
    //!
    //! Driven against a real library on disk with real files, because every claim
    //! here is about a file existing or not.

    use super::*;
    use scitadel_core::models::PaperId;
    use scitadel_core::ports::PaperRepository as _;
    use scitadel_db::sqlite::{ArtefactWrite, BlobWrite, WriteMode, blob_rel_path};

    struct Fx {
        dir: tempfile::TempDir,
        db: Database,
        paper: Paper,
    }

    impl Fx {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = Database::open(&dir.path().join("scitadel.db")).unwrap();
            db.migrate().unwrap();
            let mut paper =
                Paper::new("Deep learning for radiopharmaceutical image reconstruction");
            paper.id = PaperId::from("p-1".to_string());
            paper.year = Some(2020);
            paper.authors = vec!["Young, Christopher J.".to_string()];
            paper.doi = Some("10.99999/some.suffix.12345".to_string());
            let (repo, _, _, _, _) = db.repositories();
            repo.save(&paper).unwrap();
            // The work's own directory, as it exists for any library that has been
            // fetched into. A scan of an empty directory is a fact about the
            // library, so the fixture has one.
            std::fs::create_dir_all(dir.path().join(MANIFEST_DIR).join(file_stem_for(&paper)))
                .unwrap();
            Self { dir, db, paper }
        }

        fn root(&self) -> PathBuf {
            self.dir.path().to_path_buf()
        }

        fn manifest(&self) -> PathBuf {
            manifest_path(&self.root(), &self.paper)
        }

        /// Hold a full text for the work, bytes and all.
        fn hold_fulltext(&self) {
            let sha = format!("{:0>64}", "a1b2c3d4");
            let row = ArtefactWrite {
                sha256: Some(sha.clone()),
                ..Self::fulltext_row(&sha)
            };
            self.db
                .write_artefacts(&[row], WriteMode::Reconcile)
                .unwrap();
        }

        /// Hold a full text **with a licence**, the way a fetch through a
        /// source that stated one records it.
        fn hold_licensed_fulltext(&self, url: &str) {
            let sha = format!("{:0>64}", "c0ffee01");
            let row = ArtefactWrite {
                license_url: Some(url.to_string()),
                license_content_version: Some("vor".to_string()),
                license_start: Some("2024-01-01".to_string()),
                license_source: Some("crossref".to_string()),
                ..Self::fulltext_row(&sha)
            };
            self.db
                .write_artefacts(&[row], WriteMode::Reconcile)
                .unwrap();
        }

        /// One `fulltext_pdf` row for the fixture work, licence columns empty.
        ///
        /// The single place the row is spelled, so the two fixtures above differ
        /// by the licence and by nothing else — which is what makes the licence
        /// test a test about the licence rather than about the fixture.
        fn fulltext_row(sha: &str) -> ArtefactWrite {
            ArtefactWrite {
                id: String::new(),
                paper_id: "p-1".into(),
                kind: "fulltext_pdf".into(),
                version: "vor".into(),
                locator: String::new(),
                sha256: None,
                format: Some("pdf".into()),
                access_status: "full_text".into(),
                route: "unpaywall".into(),
                access_basis: "oa_license".into(),
                label: None,
                caption: None,
                source_url: None,
                publisher: None,
                publisher_note: None,
                license_url: None,
                license_content_version: None,
                license_start: None,
                license_source: None,
                imported_from: None,
                retrieved_at: "2026-01-01T00:00:00+00:00".into(),
                missing_on_disk: false,
                blob: Some(BlobWrite {
                    rel_path: blob_rel_path(sha, "pdf"),
                    sha256: sha.to_string(),
                    bytes: 4,
                    mime: "application/pdf".into(),
                    created_at: "2026-01-01T00:00:00+00:00".into(),
                }),
            }
        }

        /// Record a want the work does not hold.
        fn gap(&self) {
            let mut conn = self.db.conn().unwrap();
            scitadel_db::sqlite::write_acquisition_states(
                &mut conn,
                &[scitadel_db::sqlite::StateWrite {
                    paper_id: "p-1".into(),
                    kind: "si".into(),
                    locator: "supporting-information-s1".into(),
                    wanted_version: "vor".into(),
                    status: "needs_ill".into(),
                    reason: Some("no route (raid's curation verdict: pending_review)".into()),
                    publisher: None,
                    hint_url: Some("https://example.org/s1".into()),
                    drop_path: Some("/lib/papers/stem/si/S1.pdf".into()),
                    next_attempt_at: None,
                    updated_at: "2026-01-01T00:00:00+00:00".into(),
                }],
            )
            .unwrap();
        }

        /// The claim a manifest writer must hold.
        fn claim(&self) -> String {
            let owner = scitadel_db::sqlite::new_lease_owner();
            assert!(
                self.db
                    .acquire_lease("p-1", &owner, LEASE_TTL)
                    .unwrap()
                    .is_some()
            );
            owner
        }
    }

    const LEASE_TTL: i64 = scitadel_db::sqlite::LEASE_TTL_MS;

    /// #253's measure of SI: "7 of 27 'SI' files on disk were HTML landing
    /// pages." A real SI for this work.
    const SI_BYTES: &[u8] = b"%PDF-1.7\n% real supplementary data\n%%EOF\n";

    /// Put an SI file into the work's drop-in directory, the way a person would.
    fn drop_in_si(fx: &Fx, name: &str, bytes: &[u8]) -> PathBuf {
        let dir = manifest_path(&fx.root(), &fx.paper)
            .parent()
            .expect("a directory")
            .join("si");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Every answer a reader could give, derived from the database. If a manifest
    /// were being read anywhere, one of these would move when the manifest did.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Derived(String);

    fn derived(db: &Database) -> Vec<Derived> {
        let report = db.coverage_report(None).expect("coverage");
        let plan = crate::acquire::plan(db, &crate::acquire::AcquireRequest::queue())
            .expect("acquire plan");
        let artefacts = db.artefacts_for_paper("p-1").expect("artefacts");
        let scan = crate::scan::plan(db, &paper_of(db), Some(&work_dir(db))).expect("scan plan");
        vec![
            Derived(format!("missing={:?}", report.missing_total())),
            Derived(format!("entries={:?}", report.entries)),
            Derived(format!("acquire_works={:?}", plan.works)),
            Derived(format!("acquire_held={:?}", plan.held_total)),
            Derived(format!("artefacts={artefacts:?}")),
            Derived(format!("scan_counts={:?}", scan.counts)),
            Derived(format!("scan_files={:?}", scan.files)),
            Derived(format!("scan_gaps={:?}", scan.gaps)),
        ]
    }

    fn paper_of(db: &Database) -> Paper {
        let (repo, _, _, _, _) = db.repositories();
        repo.get("p-1").expect("read").expect("the paper")
    }

    fn work_dir(db: &Database) -> PathBuf {
        manifest_path(db.library_root().unwrap().unwrap().as_path(), &paper_of(db))
            .parent()
            .unwrap()
            .to_path_buf()
    }

    /// **ADR-007 §1: "It is never read as input."**
    ///
    /// Asserted three ways against three libraries that differ in one respect
    /// only — whether the mirror is absent, correct, or lying — and every
    /// database-derived answer must be identical. A single parse anywhere would
    /// show up as a difference; a reader that only looked at the mirror's
    /// *existence* would show up in the plan/write behaviour below.
    ///
    /// The poison is the informative half: it claims three artefacts the database
    /// does not have, one gap the database does not have, a work with no DOI, and
    /// a title that is somebody else's. If anything parsed it, `coverage` would
    /// report artefacts we do not hold and the plan would fetch a work we have
    /// never heard of.
    #[test]
    fn the_manifest_is_never_read_as_input() {
        let fx = Fx::new();
        fx.hold_fulltext();
        fx.gap();
        let paper = paper_of(&fx.db);

        // The truth, with no mirror on disk at all.
        let baseline = derived(&fx.db);
        assert!(
            !fx.manifest().exists(),
            "precondition: this library has no manifest yet"
        );

        // A correct mirror, written the only permitted way.
        let owner = fx.claim();
        write(&fx.db, &paper, &owner).expect("the mirror is written");
        let correct = std::fs::read_to_string(fx.manifest()).unwrap();
        assert_eq!(derived(&fx.db), baseline, "writing it changed no answer");

        // A mirror that lies about everything.
        std::fs::write(
            fx.manifest(),
            r#"{
              "work": {"paper_id": "p-does-not-exist", "stem": "somewhere-else",
                       "doi": "10.0000/fabricated", "title": "A different article entirely",
                       "year": 1999, "first_author": "Nobody, A."},
              "artefacts": [
                {"id": "forged-1", "kind": "si", "format": "pdf", "version": "vor",
                 "locator": "supporting-information-s1", "path": "blobs/ff/forged.pdf",
                 "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                 "bytes": 123456789, "route": "manual", "access_status": "full_text",
                 "access_basis": "manual", "retrieved_at": "1970-01-01T00:00:00+00:00",
                 "missing_on_disk": false},
                {"id": "forged-2", "kind": "figure", "version": "unknown", "locator": "fig-9",
                 "path": null, "sha256": null, "bytes": null, "route": "manual",
                 "access_status": "full_text", "access_basis": "manual",
                 "retrieved_at": "1970-01-01T00:00:00+00:00", "missing_on_disk": false},
                {"id": "forged-3", "kind": "table", "version": "vor", "locator": "table-1",
                 "path": "blobs/ff/forged.csv", "sha256": null, "bytes": 1, "route": "manual",
                 "access_status": "full_text", "access_basis": "manual",
                 "retrieved_at": "1970-01-01T00:00:00+00:00", "missing_on_disk": false}
              ],
              "missing": [],
              "generated_at": "1970-01-01T00:00:00+00:00"
            }"#,
        )
        .unwrap();
        assert_eq!(
            derived(&fx.db),
            baseline,
            "a manifest that lies must change no derived answer: nothing reads it"
        );

        // And gone again — the strongest form, because a reader that merely
        // checked the file existed would now be reading nothing.
        std::fs::remove_file(fx.manifest()).unwrap();
        assert_eq!(derived(&fx.db), baseline, "removing it changed no answer");

        // The correct mirror's content really does describe the committed state,
        // so "nothing reads it" is not "nothing writes it either".
        std::fs::write(fx.manifest(), &correct).unwrap();
        let mirror: Manifest = serde_json::from_str(&correct).unwrap();
        assert_eq!(mirror.work.paper_id, "p-1");
        assert_eq!(
            mirror.artefacts.len(),
            1,
            "the one artefact the database has"
        );
        assert_eq!(
            mirror.missing.len(),
            1,
            "the one gap the database has, with raid's own words preserved"
        );
        assert!(
            mirror.missing[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("pending_review")),
            "the mirror carries the reason verbatim: {:?}",
            mirror.missing[0].reason
        );
    }

    /// **ADR-007 §1: "written … after the DB commit."**
    ///
    /// The positive half: the mirror describes rows that are committed and
    /// visible to a *different* connection, which is what "after the commit"
    /// means in practice. `manifest::write` reads through the pool, so a mirror
    /// written before the commit could not contain them at all.
    #[test]
    fn the_manifest_is_written_after_the_db_commit() {
        let fx = Fx::new();
        fx.hold_fulltext();
        let owner = fx.claim();
        write(&fx.db, &fx.paper, &owner).expect("written");

        let body = std::fs::read_to_string(fx.manifest()).unwrap();
        let mirror: Manifest = serde_json::from_str(&body).unwrap();
        assert_eq!(mirror.artefacts.len(), 1);
        assert_eq!(mirror.artefacts[0].kind, "fulltext_pdf");
        let digest = format!("{:0>64}", "a1b2c3d4");
        assert_eq!(
            mirror.artefacts[0].sha256.as_deref(),
            Some(digest.as_str()),
            "the digest the committed row carries"
        );
        assert_eq!(
            mirror.artefacts[0].path.as_deref(),
            Some(blob_rel_path(&digest, "pdf").as_str()),
            "and the blob's root-relative path, never an absolute one"
        );
        assert!(
            !Path::new(mirror.artefacts[0].path.as_deref().expect("a path")).is_absolute(),
            "ADR-007 §1 \"Storage\": every stored path is relative to the root"
        );
    }

    /// The negative half, which is what "after the DB commit" buys: **a dry run
    /// commits nothing and therefore writes no mirror.**
    ///
    /// The whole directory is compared — manifest absent, `artefacts` identical
    /// row for row, `acquisition_state` identical, and no blob in the store —
    /// because "wrote no mirror" on its own is also what a run that wrote a
    /// *wrong* mirror into a different place would produce. And a dry run that
    /// took the lease would block the real run behind it, so the lease is checked
    /// too: the plan holds none.
    #[test]
    fn a_dry_run_writes_no_manifest_and_no_row() {
        let fx = Fx::new();
        drop_in_si(&fx, "S1.pdf", SI_BYTES);
        let dir = work_dir(&fx.db);

        let before_artefacts = fx.db.artefacts_for_paper("p-1").unwrap();
        assert!(before_artefacts.is_empty());
        let before_states: usize = fx
            .db
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM acquisition_state", [], |r| r.get(0))
            .unwrap();
        let before_blobs = count_files(&fx.root().join(scitadel_db::sqlite::BLOB_DIR));

        let plan = crate::scan::plan(&fx.db, &paper_of(&fx.db), Some(&dir)).expect("planned");
        assert_eq!(plan.files.len(), 1, "the plan saw the file");
        assert!(
            plan.manifest.is_none(),
            "a plan has no commit to describe, so it writes no mirror"
        );
        assert!(
            plan.lease.is_none(),
            "and takes no lease, so a real run is not blocked"
        );

        assert_eq!(fx.db.artefacts_for_paper("p-1").unwrap(), before_artefacts);
        let after_states: usize = fx
            .db
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM acquisition_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_states, before_states, "no want row either");
        assert!(!fx.manifest().exists(), "and no mirror on disk");
        assert_eq!(
            count_files(&fx.root().join(scitadel_db::sqlite::BLOB_DIR)),
            before_blobs
        );
        assert_eq!(
            fx.db.leases().unwrap().len(),
            0,
            "ADR-007 §1 \"Leases\": a plan never claims a work"
        );
    }

    /// #291's second proposal, and the reason the mirror is now honest about
    /// *which* licence: the four columns have a writer, so a recorded one is
    /// reported verbatim — and an unrecorded one is **not** reported as `null`.
    ///
    /// `null` in a published provenance document is a claim: it reads as "we
    /// looked and there is no licence". For most of this project's life that was
    /// exactly backwards, because nothing could write the columns at all. An
    /// absent key is the encoding that matches the fact, and a datasheet
    /// consumer must be able to tell the two apart before it decides what may
    /// be redistributed.
    #[test]
    fn a_recorded_licence_is_reported_and_an_absent_one_is_not_a_null() {
        // --- recorded: the four keys, verbatim ---
        let fx = Fx::new();
        fx.hold_licensed_fulltext("https://creativecommons.org/licenses/by/4.0/");
        let owner = fx.claim();
        write(&fx.db, &fx.paper, &owner).expect("written");
        let body = std::fs::read_to_string(fx.manifest()).unwrap();
        let mirror: Manifest = serde_json::from_str(&body).unwrap();
        let artefact = &mirror.artefacts[0];
        assert_eq!(
            artefact.license_url.as_deref(),
            Some("https://creativecommons.org/licenses/by/4.0/"),
            "the URL the registry gave, verbatim — a mirror that has to reconstruct \
             it cannot be checked against the registry that stated it"
        );
        assert_eq!(
            (
                artefact.license_content_version.as_deref(),
                artefact.license_start.as_deref(),
                artefact.license_source.as_deref(),
            ),
            (Some("vor"), Some("2024-01-01"), Some("crossref")),
            "and the three fields beside it, as they were stated"
        );
        // And the raw JSON carries the keys, so the file is what a reader sees.
        for needle in [
            "\"license_url\": \"https://creativecommons.org/licenses/by/4.0/\"",
            "\"license_content_version\": \"vor\"",
            "\"license_start\": \"2024-01-01\"",
            "\"license_source\": \"crossref\"",
        ] {
            assert!(body.contains(needle), "{needle} is in the mirror: {body}");
        }

        // --- not recorded: no keys at all, and no `null` standing in for one ---
        let fx = Fx::new();
        fx.hold_fulltext();
        let owner = fx.claim();
        write(&fx.db, &fx.paper, &owner).expect("written");
        let body = std::fs::read_to_string(fx.manifest()).unwrap();
        let mirror: Manifest = serde_json::from_str(&body).unwrap();
        let artefact = &mirror.artefacts[0];
        assert_eq!(
            artefact.license_url, None,
            "the reader still reports None: nothing was recorded"
        );
        for needle in [
            "\"license_url\"",
            "\"license_content_version\"",
            "\"license_start\"",
            "\"license_source\"",
        ] {
            assert!(
                !body.contains(needle),
                "{needle} must not appear at all — neither as a URL nor as a `null` \
                 that reads as \"there is no licence\": {body}"
            );
        }
        assert!(
            body.contains("\"access_basis\": \"oa_license\""),
            "and access_basis is still there, because that one *is* established: \
             it is the answer to why the fetch was lawful: {body}"
        );
    }

    fn count_files(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_file())
            .count()
    }

    /// **ADR-007 §1: "written only by the lease holder."**
    ///
    /// Both directions of the question, because a check that only refuses is also
    /// satisfied by an implementation that refuses everybody.
    #[test]
    fn the_manifest_is_written_only_by_the_lease_holder() {
        let fx = Fx::new();
        fx.hold_fulltext();
        let owner = fx.claim();

        let stranger = scitadel_db::sqlite::new_lease_owner();
        let refused = write(&fx.db, &fx.paper, &stranger).expect_err("refused");
        let message = refused.to_string();
        assert!(message.contains("lease holder"), "{message}");
        assert!(message.contains(&owner), "and names the holder: {message}");
        assert!(
            !fx.manifest().exists(),
            "so the stranger's write put nothing on disk"
        );

        write(&fx.db, &fx.paper, &owner).expect("the holder writes");
        assert!(fx.manifest().exists());

        // Once the claim is gone, nobody writes — including the previous holder.
        fx.db.release_lease("p-1", &owner).unwrap();
        write(&fx.db, &fx.paper, &owner).expect_err("nobody holds it");
        assert!(
            fx.manifest().exists(),
            "and the existing mirror is untouched"
        );
    }

    /// **ADR-007 §1: "via a temp file and a rename."**
    ///
    /// Three properties, each the reason for the mechanism rather than a nicety:
    /// the staging file is a *sibling* (so the rename is atomic rather than a
    /// copy), its name carries the pid (so two processes cannot share it), and a
    /// failed write leaves the previous mirror byte-identical.
    #[test]
    fn the_manifest_is_written_through_a_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MANIFEST_FILENAME);

        // Sibling, pid-stamped, and `.tmp`-suffixed.
        let staging = staging_path(&path);
        assert_eq!(staging.parent(), path.parent(), "same directory");
        assert_eq!(
            staging.file_name().unwrap().to_string_lossy(),
            format!("{MANIFEST_FILENAME}.{}.tmp", std::process::id())
        );

        write_atomically(&path, b"first\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\n");
        assert!(
            !staging.exists(),
            "the rename consumed the staging file: no residue"
        );

        // A second write replaces it wholesale.
        write_atomically(&path, b"second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");

        // Now make the staging write fail — by putting a *directory* where the
        // staging file goes, which is deterministic on every platform — and the
        // previous mirror must survive byte for byte.
        std::fs::create_dir_all(&staging).unwrap();
        let err = write_atomically(&path, b"third\n").expect_err("staging cannot be written");
        assert!(matches!(err, ManifestError::Io { .. }), "{err:?}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "second\n",
            "a crash mid-write leaves the previous mirror intact, because the destination is only \
             ever replaced by a complete rename"
        );
        std::fs::remove_dir_all(&staging).unwrap();
    }
}
