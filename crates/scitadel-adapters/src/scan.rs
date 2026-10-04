//! ADR-007 §1 "Manual drop-ins": `scitadel scan` and `scitadel attach`.
//!
//! > **Manual drop-ins** are reconciled only by an explicit `scitadel scan` or
//! > `scitadel attach <paper> <file> --kind …`, never as a side effect of reading.
//! >
//! > - Unknown files get `route = 'manual'`, `access_status = 'unknown'` and a
//! >   post-fetch identity check.
//! > - A file that has disappeared is flagged `missing_on_disk = 1`; the row stays.
//!
//! # Why this is an explicit command and not a side effect
//!
//! That first sentence is the interesting one. If a read of a work could file a
//! file that happened to be lying next to it, every reader would be a writer and
//! the library's contents would depend on which features a reader happened to
//! use. So `scan` and `attach` are the *only* entry points into this module, and
//! nothing else in the workspace calls them.
//!
//! # What is reused, and what differs
//!
//! `scan` walks **the same tree with the same walk** as [`crate::import_flat`]:
//! `import_flat::scan_tree` for the crawl (so two commands cannot disagree about
//! which files exist, which is the entire content of `imported_from`),
//! `import_flat::within_root` for the containment check (one implementation, for
//! the macOS reason in its docs), `import_flat::disambiguate` for locator
//! collisions, and `normalise_label` / `mime_for` / `slot_for_dir` /
//! `is_manifest` / `modified_at` for the vocabulary. A locator is part of the
//! UNIQUE key, so a second normalisation rule would file one file twice under two
//! ids.
//!
//! Four things are different, and each is an ADR sentence:
//!
//! | | `import_flat` | `scan` / `attach` |
//! |---|---|---|
//! | `route` | `import_flat` — a published tree, a fact about the past | **`manual`** — a person put it here |
//! | verification | none | **magic bytes + the size cap**, and a **post-fetch identity check** |
//! | a vanished file | the tree simply does not have it | **the recorded row is flagged `missing_on_disk = 1` and kept** |
//! | the manifest | not written | **written after the commit, by the lease holder** |
//!
//! The lease is released by a `Drop` guard rather than a line at the end of the
//! happy path: a live lease is a work **no other process can reconcile for five
//! minutes**, so leaking one on an error path costs more than the error did.
//!
//! # The verification, and the two attempt outcomes it fills in
//!
//! ADR-007 §1 "Artefact rules": *"SI only counts when its magic bytes match its
//! declared format. The raid review found 7 of 27 'SI' files on disk were HTML
//! landing pages."* [`crate::magic`] does the comparing; a refusal here is an
//! `acquisition_attempts` row with outcome **`bad_magic`** and **no artefact**.
//! ADR-007 §1 "Storage"'s size-cap table produces **`too_large`** the same way.
//! Both spellings are declared in migration 013's `outcome` comment and, like
//! `publisher` in the slice before this one, **neither had a writer anywhere in
//! the workspace**. This is that writer.
//!
//! # The identity check on a dropped-in file
//!
//! A file a person put in the library is exactly the shape ADR-007 §3's
//! post-fetch check exists for — it may be another work's supplementary file, and
//! nothing else in the pipeline would ever notice. So every row ADR-007's bullet
//! names (`access_status = 'unknown'`, which is every `si` / `table` / `figure`)
//! is checked against the stored work before it counts, and a `mismatch` files
//! nothing.
//!
//! A `unverified` verdict does **not** block, for the same reason it does not
//! block on a fetched PDF: `unverified` is no evidence at all, and blocking on it
//! would mean a supplementary workbook — which names no work anywhere — could
//! never be filed by any run, ever.
//!
//! # The lease
//!
//! One claim per work, from [`scitadel_db::sqlite::leases`], taken before any
//! file is touched and released at the end. `scan` and `attach` never renew: they
//! touch no publisher host, so there is no paced fetch that could outlive a
//! five-minute claim. `acquire`, which does fetch, renews while it fetches.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use scitadel_core::models::Paper;
use scitadel_db::sqlite::{
    ACCESS_BASIS_MANUAL, ArtefactWrite, AttemptWrite, BlobWrite, Database, FULLTEXT_LOCATOR,
    IdentityPhase, IdentitySource, StateWrite, VERSION_UNKNOWN, WriteMode, blob_rel_path,
    cap_for_kind, fulltext_kind, hash_file, new_lease_owner, store_blob,
};
use serde::Serialize;

use crate::download::file_stem_for;
use crate::error::AdapterError;
use crate::identity::{self, Verdict, WorkIdentity};
use crate::magic::{self, MagicVerdict};
use crate::manifest::ManifestWrite;
use crate::{import_flat, manifest};

/// `route` for a file a person dropped in (ADR-007 §1: "Unknown files get
/// `route = 'manual'`").
pub const ROUTE_MANUAL: &str = "manual";

/// The `acquisition_state.kind` a full-text artefact's want uses — the ladder's
/// vocabulary, which is one coarser than `artefacts.kind` on purpose (a want is
/// "a full text", not "a PDF").
const WANT_FULLTEXT: &str = "fulltext";

/// The file `scan` tells a person to drop a full text at.
const FULLTEXT_FILENAME: &str = "fulltext.pdf";

/// How long these commands hold a work's claim. Same default as every other
/// lease in the workspace, so a row written by one is readable by another.
pub const LEASE_TTL_MS: i64 = scitadel_db::sqlite::LEASE_TTL_MS;

/// The `artefacts.kind` values `scitadel attach --kind` accepts.
///
/// `fulltext` is not an artefact kind — it is the want — and it resolves through
/// the file's extension, which is why it is accepted here and named separately in
/// [`resolve_kind`].
pub const ATTACH_KINDS: [&str; 7] = [
    "fulltext",
    "fulltext_pdf",
    "fulltext_html",
    "fulltext_xml",
    "si",
    "table",
    "figure",
];

/// One artefact a `scan` / `attach` run decided something about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileOutcome {
    /// As walked, or as named on the command line, resolved.
    pub path: PathBuf,
    pub kind: String,
    pub locator: String,
    /// `bad_magic`, `too_large`, `identity_mismatch`, or `None` when the file was
    /// filed. The values are `acquisition_attempts.outcome` spellings, not
    /// invented ones.
    pub refused_because: Option<String>,
    /// The reason, which is also the `acquisition_attempts.detail` recorded.
    pub detail: Option<String>,
}

/// A wanted-but-absent artefact a run recorded, and where to close it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedGap {
    pub kind: String,
    pub locator: String,
    pub drop_path: PathBuf,
    pub reason: String,
}

/// The verdict of the post-fetch identity check on one dropped-in file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManualIdentity {
    pub path: PathBuf,
    /// An ADR-007 §3 status spelling — `ok`, `mismatch`, `unverified`,
    /// `overridden` — or `not_applicable`, which is **not** one of them and means
    /// the bytes named no title at all, so nothing was compared. Deliberately
    /// distinct from `unverified`: that says "we looked and nothing corroborated
    /// it", this says "there was nothing to look at".
    pub status: String,
    pub expected_title: Option<String>,
    pub resolved_title: Option<String>,
    pub score: Option<f64>,
}

/// What one `scan` run did.
///
/// The refusal lists are the point of the report: a file that was **not** filed,
/// and why it was not, is the information a person needs — and a report that only
/// listed successes would look identical to a clean run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanReport {
    pub paper_id: String,
    pub paper_id_short: String,
    /// The directory that was walked, resolved.
    pub root: PathBuf,
    /// Artefact rows recorded, by `kind`. Counts what this run wrote, so a re-run
    /// over an unchanged directory is visibly quiet.
    pub counts: BTreeMap<String, usize>,
    /// Distinct blobs copied into the store by this run.
    pub blobs: usize,
    /// Every file the run decided something about, filed or not.
    pub files: Vec<FileOutcome>,
    /// Recorded rows whose file has disappeared. The rows stay.
    pub vanished: Vec<String>,
    pub gaps: Vec<RecordedGap>,
    /// The identity checks taken on this run's files.
    pub identity: Vec<ManualIdentity>,
    /// The manifest, when one was written.
    pub manifest: Option<ManifestWrite>,
    /// `Some(owner)` when this run claimed the work.
    pub lease: Option<String>,
    /// `true` when another live process held the lease, so nothing was written.
    pub leased_elsewhere: bool,
}

/// What one `attach` did. Its own type rather than a one-element [`ScanReport`],
/// because the interesting assertion about an attach is a single file's.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AttachReport {
    pub paper_id: String,
    pub paper_id_short: String,
    pub file: FileOutcome,
    /// The identity check taken on the file, when one applied.
    pub identity: Option<ManualIdentity>,
    /// Gap rows this file closed.
    pub retracted: usize,
    /// `true` when the file's `imported_from` resolves **inside** this library,
    /// which is what makes the recorded provenance survive the library moving.
    pub inside_library: bool,
    pub manifest: Option<ManifestWrite>,
    pub lease: Option<String>,
}

/// Anything that stops a `scan` / `attach`.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error(
        "scan / attach need a file-backed library: an in-memory database has no root, so it can \
         hold no blobs (ADR-007 §1 \"Storage\")"
    )]
    NoLibraryRoot,
    #[error("no such directory: {0}")]
    NoSuchDirectory(PathBuf),
    #[error("no such file: {0}")]
    NoSuchFile(PathBuf),
    #[error(
        "another live process holds the lease on {paper_id}, so nothing was written; try \
             again once it finishes"
    )]
    LeasedElsewhere { paper_id: String },
    #[error(transparent)]
    Db(#[from] scitadel_db::error::DbError),
    #[error(transparent)]
    Manifest(#[from] manifest::ManifestError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
}

/// A work's claim, released however this function returns.
///
/// Without this, every early return between the claim and the end of the walk
/// would leave a live lease behind — and a live lease is a work that **no other
/// process can reconcile for five minutes**. That is a much worse outcome than
/// the error that caused the early return, so the release is a `Drop` rather than
/// a line at the bottom of the happy path.
struct Claim {
    db: Database,
    paper_id: String,
    owner: String,
}

impl Claim {
    /// Claim `paper_id`, or `None` when a live process already holds it.
    fn take(db: &Database, paper_id: &str) -> Result<Option<Self>, scitadel_db::error::DbError> {
        let owner = new_lease_owner();
        Ok(db
            .acquire_lease(paper_id, &owner, LEASE_TTL_MS)?
            .map(|_| Self {
                db: db.clone(),
                paper_id: paper_id.to_string(),
                owner,
            }))
    }

    /// Claim `paper_id` or refuse: for `attach`, where the manifest cannot be
    /// regenerated by anybody else, so proceeding would leave the library with
    /// rows and a stale mirror.
    fn require(db: &Database, paper_id: &str) -> Result<Self, ScanError> {
        Self::take(db, paper_id)?.ok_or_else(|| ScanError::LeasedElsewhere {
            paper_id: paper_id.to_string(),
        })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Err(e) = self.db.release_lease(&self.paper_id, &self.owner) {
            tracing::warn!(
                paper_id = %self.paper_id,
                error = %e,
                "could not release the work's lease; it expires on its own"
            );
        }
    }
}

/// Reconcile a directory of manually dropped-in files against the database.
///
/// `root` is the work's own directory (`<library_root>/papers/<stem>/`) when
/// `None` — where ADR-007's drop-in files land — or an explicit directory to
/// reconcile instead.
///
/// **Nothing else in the workspace does this**, per the ADR: reading a work never
/// files a file that happens to be lying beside it. That is why this takes an
/// explicit directory rather than deriving one from a read.
///
/// Idempotent in the sense that matters: an unchanged directory **writes no
/// artefact row and no want row**, because artefact ids are derived from the UNIQUE
/// key, `retrieved_at` comes from the file's own mtime, and [`WriteMode::Reconcile`]
/// updates a row only where a value differs. The manifest mirror *is* rewritten,
/// because its `generated_at` is the time of generation — see
/// [`crate::manifest`].
///
/// # Errors
///
/// [`ScanError::NoLibraryRoot`], [`ScanError::NoSuchDirectory`] when the
/// directory is gone (a fact about the library, not a crash),
/// [`ScanError::LeasedElsewhere`] when a live process holds the work, and
/// SQLite or file-system failures. **No per-file trouble is an error**: an
/// unreadable file becomes a flagged row and a refused path becomes a line in the
/// report, so one bad file cannot abort a whole library.
#[allow(clippy::too_many_lines)]
pub fn scan(db: &Database, paper: &Paper, root: Option<&Path>) -> Result<ScanReport, ScanError> {
    let library_root = db.library_root()?.ok_or(ScanError::NoLibraryRoot)?;
    let paper_id = paper.id.as_str().to_string();

    // Claim the work before touching a single file. A live holder is not a
    // failure — it is the reason the lease exists — so the run reports the fact
    // and writes nothing.
    let Some(claim) = Claim::take(db, &paper_id)? else {
        return Ok(leased_elsewhere(paper, root));
    };

    let paper_dir = match root {
        // An explicit root is taken whole, like `import_flat`'s: a directory
        // that exists is the directory the caller meant.
        Some(root) if !root.is_dir() => return Err(ScanError::NoSuchDirectory(root.to_path_buf())),
        Some(root) => root.to_path_buf(),
        None => library_root
            .join(manifest::MANIFEST_DIR)
            .join(file_stem_for(paper)),
    };
    if !paper_dir.is_dir() {
        return Err(ScanError::NoSuchDirectory(paper_dir));
    }
    // One resolved root for the whole walk, so every containment check compares
    // like with like — `import_flat`'s rule, and the macOS bug it exists for.
    let canonical_root = import_flat::resolve_best_effort(&paper_dir);

    let tree = import_flat::scan_tree(&paper_dir, &canonical_root, &file_stem_for(paper));
    let candidates = import_flat::disambiguate(tree.candidates);

    let now = Utc::now();
    // Phase 1 — file work with no write lock held: read, check, hash, copy into
    // the store. The same split as the backfill, the importer and the ladder, so
    // hashing a 250 MB supplement never blocks another process's write.
    let planned: Vec<Planned> = candidates
        .iter()
        .map(|candidate| plan_one(db, &paper_id, candidate, &library_root, now))
        .collect();

    // Phase 2 — one short transaction for the artefact rows, and only for the
    // files that passed every check.
    let rows: Vec<ArtefactWrite> = planned.iter().filter_map(|p| p.row.clone()).collect();
    db.write_artefacts(&rows, WriteMode::Reconcile)?;

    // The post-fetch identity check, for exactly the rows ADR-007's bullet names
    // (`access_status = 'unknown'`). After the commit, because the evidence is
    // worth keeping even when the *row* is not: the blob is already in the store,
    // and `paper_identity_checks` is where both titles go.
    let mut files: Vec<FileOutcome> = planned.iter().map(Planned::outcome).collect();
    let mut identity = Vec::new();
    for (index, planned) in planned.iter().enumerate() {
        let Some(row) = planned.row.as_ref() else {
            continue;
        };
        if row.access_status != "unknown" {
            continue;
        }
        let check = check_dropped_identity(db, paper, &planned.path)?;
        if check.status == "mismatch" {
            withdraw_row(db, row)?;
            files[index].refused_because = Some("identity_mismatch".to_string());
            files[index].detail = Some(mismatch_reason(&check));
            record_attempt(
                db,
                &paper_id,
                want_kind_for(&row.kind),
                &row.locator,
                "identity_mismatch",
                now,
                &mismatch_reason(&check),
            );
        }
        identity.push(check);
    }

    // A recorded file that has disappeared: flagged, and the row stays.
    let vanished = flag_vanished(db, &paper_id)?;

    let gaps = record_gap(db, &paper_id, &canonical_root, &rows, now)?;
    let blobs = scitadel_db::sqlite::distinct_blob_count(rows.iter());

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for row in &rows {
        *counts.entry(row.kind.clone()).or_insert(0) += 1;
    }

    // Last, after every commit above: the mirror. A failure to write it is
    // reported as `None` rather than failing the run — the database is right, and
    // the mirror is regenerable, so the next lease holder repairs it.
    let mirror = manifest::write(db, paper, &claim.owner).ok();

    Ok(ScanReport {
        paper_id,
        paper_id_short: paper.id.short().to_string(),
        root: canonical_root,
        counts,
        blobs,
        files,
        vanished,
        gaps,
        identity,
        manifest: mirror,
        lease: Some(claim.owner.clone()),
        leased_elsewhere: false,
    })
}

/// Decide what [`scan`] would do, and write nothing.
///
/// The `--dry-run` shape, and it is a **plan**: no row, no blob copy, no lease,
/// no manifest, no filesystem write beyond the reads. That matters for the same
/// reason `--dry-run` exists in [`crate::acquire`] — a dry run whose answer cannot
/// be checked against a table that did not move is decoration, and a lease taken
/// "just to look" would make a dry run block a real run.
///
/// The refusals are the interesting output: a file whose bytes are an HTML
/// landing page gets the same `bad_magic` verdict and the same reason a real run
/// would record, so the plan is a faithful preview of the `attempts` table.
///
/// Never takes a lease, so a plan works while another process is mid-fetch.
pub fn plan(db: &Database, paper: &Paper, root: Option<&Path>) -> Result<ScanReport, ScanError> {
    let library_root = db.library_root()?.ok_or(ScanError::NoLibraryRoot)?;
    let paper_id = paper.id.as_str().to_string();
    let paper_dir = match root {
        Some(root) if !root.is_dir() => return Err(ScanError::NoSuchDirectory(root.to_path_buf())),
        Some(root) => root.to_path_buf(),
        None => library_root
            .join(manifest::MANIFEST_DIR)
            .join(file_stem_for(paper)),
    };
    let canonical_root = import_flat::resolve_best_effort(&paper_dir);
    let tree = import_flat::scan_tree(&paper_dir, &canonical_root, &file_stem_for(paper));
    let candidates = import_flat::disambiguate(tree.candidates);

    let mut files = Vec::with_capacity(candidates.len());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut holds_fulltext = false;
    for candidate in &candidates {
        if candidate.kind.starts_with("fulltext") {
            holds_fulltext = true;
        }
        let (refused_because, detail) = preview(candidate);
        if refused_because.is_none() {
            *counts.entry(candidate.kind.to_string()).or_insert(0) += 1;
        }
        files.push(FileOutcome {
            path: candidate.path.clone(),
            kind: candidate.kind.to_string(),
            locator: candidate.locator.clone(),
            refused_because,
            detail,
        });
    }

    let gaps = if holds_fulltext || db.has_fulltext_artefact(&paper_id)? {
        Vec::new()
    } else {
        vec![RecordedGap {
            kind: WANT_FULLTEXT.to_string(),
            locator: FULLTEXT_LOCATOR.to_string(),
            drop_path: canonical_root.join(FULLTEXT_FILENAME),
            reason: format!(
                "no fulltext.pdf / fulltext.html was found in the reconciled directory {}",
                canonical_root.display()
            ),
        }]
    };

    Ok(ScanReport {
        paper_id,
        paper_id_short: paper.id.short().to_string(),
        root: canonical_root,
        counts,
        blobs: 0,
        files,
        vanished: Vec::new(),
        gaps,
        identity: Vec::new(),
        manifest: None,
        lease: None,
        leased_elsewhere: false,
    })
}

/// What a real run would decide about one candidate, without deciding it.
///
/// The two refusals mirror the size and magic arms of [`plan_one`], and the "gone"
/// arm is the one a plan can see and a report must say: the walk listed the file,
/// so the plan says a real run would keep the row and flag it.
fn preview(candidate: &import_flat::Candidate) -> (Option<String>, Option<String>) {
    let path = &candidate.path;
    let Ok(meta) = std::fs::metadata(path) else {
        return (
            None,
            Some(
                "the file is gone or unreadable; a real run keeps the row and flags \
                 missing_on_disk"
                    .to_string(),
            ),
        );
    };
    if let Some(cap) = cap_for_kind(candidate.kind)
        && meta.len() as i64 > cap
    {
        return (
            Some("too_large".to_string()),
            Some(format!(
                "{} is over the {} cap ADR-007 §1 \"Storage\" sets for a {} artefact",
                human_bytes(meta.len() as i64),
                human_bytes(cap),
                candidate.kind
            )),
        );
    }
    let declared = candidate.format.clone().unwrap_or_default();
    let head = magic::read_head(path).unwrap_or_default();
    let verdict = magic::check(&declared, &head);
    if verdict.is_mismatch() {
        return (
            Some("bad_magic".to_string()),
            verdict.reason().map(str::to_string),
        );
    }
    (None, None)
}

/// File one dropped-in file against `paper` under an explicit `kind`.
///
/// The counterpart to [`scan`] for the common case where a person knows exactly
/// which work a file belongs to.
///
/// `kind` is one of [`ATTACH_KINDS`]. `fulltext` resolves through the file's
/// extension, and a *specific* full-text kind must agree with it — filing a real
/// HTML page as `fulltext_pdf` is the exact defect [`crate::magic`] exists to
/// catch, and letting it in through the command line would make that check
/// decorative.
///
/// `label` is the human name (`Supporting Information S1`); the stable `locator`
/// is derived from it with [`import_flat::normalise_label`] unless `locator` is
/// given explicitly.
#[allow(clippy::too_many_lines)]
pub fn attach(
    db: &Database,
    paper: &Paper,
    file: &Path,
    kind: &str,
    label: Option<&str>,
    locator: Option<&str>,
) -> Result<AttachReport, ScanError> {
    let library_root = db.library_root()?.ok_or(ScanError::NoLibraryRoot)?;
    let paper_id = paper.id.as_str().to_string();
    // `require`, not `take`: unlike a scan — which reports "another process holds
    // this" and stops — an `attach` that proceeded would write an artefact row
    // and then be unable to regenerate the mirror, leaving the library with rows
    // and a stale provenance document. So it refuses up front, and the refusal is
    // recoverable by running it again.
    let claim = Claim::require(db, &paper_id)?;

    let resolved = resolve_file(file)?;
    let kind = resolve_kind(kind, &resolved)?;
    let ext = scitadel_db::sqlite::file_extension(&resolved.to_string_lossy());
    let label = label.map_or_else(
        || {
            resolved
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
        },
        str::to_string,
    );
    let locator = locator.map_or_else(
        || {
            if kind.starts_with("fulltext") {
                FULLTEXT_LOCATOR.to_string()
            } else {
                import_flat::normalise_label(&label)
            }
        },
        str::to_string,
    );

    // Whether the file is inside this library is worth deciding rather than
    // assuming: it is what makes the recorded `imported_from` a path that still
    // resolves after the library moves, and a person attaching from
    // `~/Downloads` should be told the provenance is outside the library.
    let inside_library = import_flat::within_root(&library_root, &resolved);

    let candidate = import_flat::Candidate {
        path: resolved.clone(),
        has_bytes: true,
        // The slot decides `access_status` (`full_text` for the full-text slot,
        // `unknown` for everything else) and therefore whether ADR-007 §1's
        // post-fetch check runs — so it has to come from the resolved kind, not
        // from the fact that `attach` was handed one path.
        slot: slot_for(kind),
        kind,
        label,
        locator,
        format: (!ext.is_empty()).then(|| ext.clone()),
        caption: None,
    };
    let now = Utc::now();
    let planned = plan_one(db, &paper_id, &candidate, &library_root, now);
    let mut outcome = planned.outcome();

    // One short transaction, exactly as `scan` does.
    let mut retracted = 0;
    let mut check = None;
    if let Some(row) = planned.row.clone() {
        db.write_artefacts(std::slice::from_ref(&row), WriteMode::Reconcile)?;
        // The drop-path gap this file just closed. Scoped by `(kind, locator)`
        // and by `drop_path IS NOT NULL`, so the ladder's own `pending` row —
        // which a fetch closes, not a drop-in — is left exactly as it is.
        retracted += db.retract_drop_gaps(&paper_id, want_kind_for(&row.kind), &row.locator)?;
        if row.access_status == "unknown" {
            let verdict = check_dropped_identity(db, paper, &resolved)?;
            if verdict.status == "mismatch" {
                withdraw_row(db, &row)?;
                outcome.refused_because = Some("identity_mismatch".to_string());
                outcome.detail = Some(mismatch_reason(&verdict));
                record_attempt(
                    db,
                    &paper_id,
                    want_kind_for(&row.kind),
                    &row.locator,
                    "identity_mismatch",
                    now,
                    &mismatch_reason(&verdict),
                );
                // The file did not arrive, so the gap it would have closed stands.
                retracted = 0;
            }
            check = Some(verdict);
        }
    }

    let mirror = manifest::write(db, paper, &claim.owner).ok();

    Ok(AttachReport {
        paper_id,
        paper_id_short: paper.id.short().to_string(),
        file: outcome,
        identity: check,
        retracted,
        inside_library,
        manifest: mirror,
        lease: Some(claim.owner.clone()),
    })
}

/// A [`ScanReport`] for a work another live process is holding.
fn leased_elsewhere(paper: &Paper, root: Option<&Path>) -> ScanReport {
    ScanReport {
        paper_id: paper.id.as_str().to_string(),
        paper_id_short: paper.id.short().to_string(),
        root: root.map_or_else(|| PathBuf::from("<not scanned>"), Path::to_path_buf),
        counts: BTreeMap::new(),
        blobs: 0,
        files: Vec::new(),
        vanished: Vec::new(),
        gaps: Vec::new(),
        identity: Vec::new(),
        manifest: None,
        lease: None,
        leased_elsewhere: true,
    }
}

/// Phase 1 for one file: check it, hash it, copy it into the store, build the
/// row.
///
/// Every refusal is decided here, before anything is persisted, and each records
/// an `acquisition_attempts` row with the outcome migration 013 declares. The
/// order is ADR-007 §1 "Storage"'s: **size, then magic bytes, then the hash** — a
/// 2 GB file must not be hashed to find out it was never going to be filed, and a
/// file whose bytes are a landing page must not enter the store at all.
struct Planned {
    path: PathBuf,
    kind: String,
    locator: String,
    /// `None` when the file was refused. Also `None` for nothing else: a
    /// caption-only figure reference *has* a row, with `sha256 IS NULL`.
    row: Option<ArtefactWrite>,
    refused_because: Option<String>,
    detail: Option<String>,
}

impl Planned {
    fn outcome(&self) -> FileOutcome {
        FileOutcome {
            path: self.path.clone(),
            kind: self.kind.clone(),
            locator: self.locator.clone(),
            refused_because: self.refused_because.clone(),
            detail: self.detail.clone(),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn plan_one(
    db: &Database,
    paper_id: &str,
    candidate: &import_flat::Candidate,
    library_root: &Path,
    now: DateTime<Utc>,
) -> Planned {
    let want_kind = want_kind_for(candidate.kind);
    let full_text = candidate.slot == import_flat::Slot::FullText;
    let mut planned = Planned {
        path: candidate.path.clone(),
        kind: candidate.kind.to_string(),
        locator: candidate.locator.clone(),
        row: None,
        refused_because: None,
        detail: None,
    };

    let row = ArtefactWrite {
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
        // ADR-007 §1 "Manual drop-ins": "Unknown files get … `access_status =
        // 'unknown'`". The full-text slot is the one exception, and for
        // `import_flat`'s reason: those bytes *are* the full text, so reporting
        // them as `unknown` would make `coverage` claim we hold something we
        // cannot classify.
        access_status: if full_text { "full_text" } else { "unknown" }.to_string(),
        route: ROUTE_MANUAL.to_string(),
        access_basis: ACCESS_BASIS_MANUAL.to_string(),
        label: (!full_text).then(|| candidate.label.clone()),
        caption: candidate.caption.clone(),
        source_url: None,
        publisher: None,
        publisher_note: None,
        imported_from: Some(candidate.path.to_string_lossy().into_owned()),
        retrieved_at: import_flat::modified_at(&candidate.path)
            .unwrap_or(now)
            .to_rfc3339(),
        missing_on_disk: false,
        blob: None,
    };

    if !candidate.has_bytes {
        // A caption-only figure reference: `sha256 IS NULL` by design, and no
        // bytes to verify.
        planned.row = Some(row);
        return planned;
    }

    // ---- size cap: ADR-007 §1 "Storage" ----
    let bytes = match std::fs::metadata(&candidate.path) {
        Ok(meta) => meta.len() as i64,
        Err(e) => {
            // Gone or unreadable between the walk and here. The row stays, with
            // `missing_on_disk`: a path we recorded is a fact even when the file
            // is not there any more.
            tracing::warn!(
                path = %candidate.path.display(),
                error = %e,
                "a dropped-in file vanished mid-scan; recorded as missing_on_disk"
            );
            planned.row = Some(flagged(row));
            return planned;
        }
    };
    if let Some(cap) = cap_for_kind(candidate.kind)
        && bytes > cap
    {
        let reason = format!(
            "{bytes} bytes is over the {} cap ADR-007 §1 \"Storage\" sets for a {} artefact \
             ({cap} bytes), so no artefact is created",
            human_bytes(cap),
            candidate.kind
        );
        planned.refused_because = Some("too_large".to_string());
        planned.detail = Some(reason.clone());
        record_attempt(
            db,
            paper_id,
            want_kind,
            &row.locator,
            "too_large",
            now,
            &reason,
        );
        return planned;
    }

    // ---- magic bytes: ADR-007 §1 "Artefact rules" ----
    let head = match magic::read_head(&candidate.path) {
        Ok(head) => head,
        Err(e) => {
            tracing::warn!(
                path = %candidate.path.display(),
                error = %e,
                "could not read a dropped-in file's first bytes; recorded as missing_on_disk"
            );
            planned.row = Some(flagged(row));
            return planned;
        }
    };
    let declared = candidate.format.clone().unwrap_or_default();
    let verdict = magic::check(&declared, &head);
    if let MagicVerdict::Mismatch { reason, .. } = &verdict {
        planned.refused_because = Some("bad_magic".to_string());
        planned.detail = Some(reason.clone());
        record_attempt(
            db,
            paper_id,
            want_kind,
            &row.locator,
            "bad_magic",
            now,
            reason,
        );
        return planned;
    }
    if let MagicVerdict::Unknown { reason, .. } = &verdict {
        // Not a refusal, and deliberately not silent: the check could not confirm
        // this file, and a reader six months from now should be able to see that
        // rather than assume it was confirmed.
        tracing::debug!(
            path = %candidate.path.display(),
            reason,
            "magic bytes could not confirm the declared format; filed anyway"
        );
    }

    // ---- hash and store ----
    let ext = declared;
    let (sha256, byte_len) = match hash_file(&candidate.path) {
        Ok(hashed) => hashed,
        Err(e) => {
            tracing::warn!(
                path = %candidate.path.display(),
                error = %e,
                "a dropped-in file could not be hashed; recorded as missing_on_disk"
            );
            planned.row = Some(flagged(row));
            return planned;
        }
    };
    let mut stored = row;
    stored.sha256 = Some(sha256.clone());
    stored.blob = Some(BlobWrite {
        rel_path: blob_rel_path(&sha256, &ext),
        sha256,
        bytes: byte_len,
        mime: import_flat::mime_for(&ext).to_string(),
        created_at: now.to_rfc3339(),
    });
    // A blob we identified but could not place is a fact to flag, not a reason to
    // lose the row: every later run retries the copy and the file is untouched.
    if let Err(e) = store_blob(&candidate.path, &sha256_for(&stored), &ext, library_root) {
        tracing::warn!(
            path = %candidate.path.display(),
            error = %e,
            "hashed a dropped-in file but could not copy it into the blob store"
        );
        planned.row = Some(flagged(stored));
        return planned;
    }
    planned.row = Some(stored);
    planned
}

/// The digest of a planned row, which `store_blob` needs and which is always
/// present by the time this is called.
fn sha256_for(row: &ArtefactWrite) -> String {
    row.sha256.clone().unwrap_or_else(|| "unhashed".to_string())
}

/// An artefact row whose file could not be read or placed.
///
/// `missing_on_disk` is the schema's own word for "the path is recorded and we
/// hold no usable bytes for it", and the row stays either way.
fn flagged(mut row: ArtefactWrite) -> ArtefactWrite {
    row.missing_on_disk = true;
    row
}

/// The `acquisition_attempts.kind` for an `artefacts.kind`.
///
/// `acquisition_state.kind` is the want vocabulary (`fulltext`, `si`, `table`,
/// `figure`) and `acquisition_attempts.kind` is free TEXT, so it records the want
/// rather than the artefact kind — which is what makes an attempt row about a
/// refused SI readable as "we tried to get the SI for this work".
fn want_kind_for(artefact_kind: &str) -> &'static str {
    match artefact_kind {
        "fulltext_pdf" | "fulltext_html" | "fulltext_xml" => WANT_FULLTEXT,
        "si" => "si",
        "table" => "table",
        "figure" => "figure",
        _ => "unknown",
    }
}

/// Record one refusal in `acquisition_attempts`.
///
/// The outcome spellings are migration 013's own, and `record_attempt` refuses
/// anything outside that list — so a typo here is a hard error rather than a row
/// no reader can interpret.
///
/// Best-effort about the write itself: the refusal is already in this run's
/// report, and an audit-row failure must not turn "this file was not filed" into
/// "the scan aborted". Logged loudly instead.
fn record_attempt(
    db: &Database,
    paper_id: &str,
    kind: &str,
    locator: &str,
    outcome: &str,
    now: DateTime<Utc>,
    detail: &str,
) {
    let attempt = AttemptWrite {
        paper_id: Some(paper_id.to_string()),
        kind: kind.to_string(),
        locator: locator.to_string(),
        route: ROUTE_MANUAL.to_string(),
        // No bucket: a file a person put on disk never spent a pacing permit, and
        // naming a bucket here would attribute a refusal to a publisher.
        bucket: None,
        started_at: now.to_rfc3339(),
        outcome: outcome.to_string(),
        http_status: None,
        detail: Some(detail.to_string()),
    };
    if let Err(e) = db.record_attempt(&attempt) {
        tracing::warn!(error = %e, "could not record a refusal in acquisition_attempts");
    }
}

/// Withdraw an artefact row whose bytes the identity check refused.
///
/// **Only ever called for a row this run wrote**, because
/// [`WriteMode::Reconcile`] already wrote it in the phase-2 transaction and the
/// check runs afterwards. Scoped by the deterministic artefact id, so no other
/// artefact of the work can be caught by the sweep.
fn withdraw_row(db: &Database, row: &ArtefactWrite) -> Result<(), ScanError> {
    let id = scitadel_db::sqlite::artefact_id(&row.paper_id, &row.kind, &row.version, &row.locator);
    let conn = db.conn()?;
    let deleted = conn
        .execute("DELETE FROM artefacts WHERE id = ?1", [&id])
        .map_err(scitadel_db::error::DbError::from)?;
    if deleted == 1 {
        tracing::warn!(
            paper_id = %row.paper_id,
            kind = %row.kind,
            locator = %row.locator,
            "a dropped-in file was refused by the identity check; its artefact row was withdrawn"
        );
    }
    Ok(())
}

/// Why a file's own title did not match the work it was dropped in beside.
fn mismatch_reason(check: &ManualIdentity) -> String {
    format!(
        "the file's own title ({}) does not match this work's ({}), so nothing was filed — \
         ADR-007 §3 blocks filing under a mismatched identity, and \
         `scitadel override-identity` settles it by hand",
        check.resolved_title.as_deref().unwrap_or("(none)"),
        check.expected_title.as_deref().unwrap_or("(none)")
    )
}

/// The ADR-007 §3 post-fetch identity check for one dropped-in file.
///
/// The expectation is the **stored work's** own identity, because a manual drop-in
/// has no registry behind it: there is no resolved metadata other than the paper
/// row, and inventing a second one would be a guess. This is the weaker of the
/// two checks, and the row says which expectation was used by carrying both
/// titles.
///
/// A `mismatch` is the only blocking verdict, and only because it is positive
/// evidence — see the module docs.
fn check_dropped_identity(
    db: &Database,
    paper: &Paper,
    path: &Path,
) -> Result<ManualIdentity, ScanError> {
    let expected = WorkIdentity {
        title: Some(paper.title.clone()),
        year: paper.year,
        first_author: paper.authors.first().cloned(),
    };
    let not_applicable = || ManualIdentity {
        path: path.to_path_buf(),
        // Not `unverified`: this says the bytes named no title at all, so there
        // was nothing to compare.
        status: "not_applicable".to_string(),
        expected_title: expected.title.clone(),
        resolved_title: None,
        score: None,
    };
    let Some(bytes) = read_for_identity(path)? else {
        return Ok(not_applicable());
    };
    let Some(served) = identity::served_identity(&bytes) else {
        return Ok(not_applicable());
    };

    let checked = identity::verify(&expected, &served);
    let status = match checked.verdict {
        Verdict::Ok => "ok",
        Verdict::Mismatch => "mismatch",
        Verdict::Unverified => "unverified",
    };
    // A person who settled this work's identity wins outright, and the machine's
    // verdict is not recorded at all — the same write-once rule the fetch path
    // obeys, so `scan` cannot re-litigate a decision `override-identity` made.
    let settled = db
        .latest_identity_check(paper.id.as_str(), IdentityPhase::PostFetch)
        .ok()
        .flatten()
        .filter(|existing| existing.status.is_overridden());
    let status = if let Some(existing) = settled {
        tracing::info!(
            paper_id = %paper.id.as_str(),
            reason = %existing.override_reason.as_deref().unwrap_or("(none recorded)"),
            "identity check skipped: a person has already settled this work's identity"
        );
        "overridden"
    } else {
        let write = checked.to_write(
            paper.id.as_str().to_string(),
            IdentityPhase::PostFetch,
            IdentitySource::ServedPage,
            expected.title.clone(),
            served.title.clone(),
            Utc::now().to_rfc3339(),
        );
        if let Err(e) = db.write_identity_check(&write) {
            tracing::warn!(error = %e, "could not record a manual-file identity check");
        }
        status
    };

    Ok(ManualIdentity {
        path: path.to_path_buf(),
        status: status.to_string(),
        expected_title: expected.title.clone(),
        resolved_title: served.title.clone(),
        score: checked.score,
    })
}

/// The bytes an identity check reads: all of a small file, or its head.
///
/// Capped at a megabyte, because a supplementary workbook can be 250 MB and the
/// question being asked — "what does this file say it is?" — is answered by its
/// first kilobyte. A file whose title is beyond the cap reads as no title, which
/// is `not_applicable` and therefore not a block.
fn read_for_identity(path: &Path) -> Result<Option<Vec<u8>>, ScanError> {
    const MAX_IDENTITY_BYTES: u64 = 1024 * 1024;
    let unreadable = |e: std::io::Error| {
        ScanError::Adapter(AdapterError::Other(format!("{}: {e}", path.display())))
    };
    let meta = std::fs::metadata(path).map_err(unreadable)?;
    if meta.len() <= MAX_IDENTITY_BYTES {
        return std::fs::read(path).map(Some).map_err(unreadable);
    }
    magic::read_head(path)
        .map(Some)
        .map_err(|e| ScanError::Adapter(AdapterError::Other(format!("{}: {e}", path.display()))))
}

/// Flag every recorded `manual` artefact whose file has disappeared — the row
/// stays.
///
/// ADR-007 §1: "A file that has disappeared is flagged `missing_on_disk = 1`; the
/// row stays." The row staying is the load-bearing half: it is the record that the
/// work *had* this file, recorded at a path, and that it is gone — which is what
/// `coverage` needs in order to report the work as needing it again.
///
/// Scoped to `route = 'manual'`, for one reason: a `legacy` or `import_flat` row's
/// `imported_from` names a path in somebody else's published tree, and "that file
/// is gone" says nothing about whether we still hold the bytes — we do, in the
/// blob store. A `manual` row's `imported_from` is where a person put the file
/// *for us*, so its absence is a fact about this library.
fn flag_vanished(db: &Database, paper_id: &str) -> Result<Vec<String>, ScanError> {
    let mut vanished = Vec::new();
    let mut flagged_rows: Vec<ArtefactWrite> = Vec::new();
    for row in db.artefacts_for_paper(paper_id)? {
        if row.route != ROUTE_MANUAL {
            continue;
        }
        let Some(imported_from) = row.imported_from.as_deref() else {
            continue;
        };
        if import_flat::resolve_best_effort(Path::new(imported_from)).exists() {
            continue;
        }
        vanished.push(format!("{}::{}", row.kind, row.locator));
        if row.missing_on_disk {
            // Already flagged: naming it again is the honest report, rewriting
            // it is not, and `Reconcile`'s `WHERE` guard would skip it anyway.
            continue;
        }
        flagged_rows.push(ArtefactWrite {
            id: String::new(),
            paper_id: row.paper_id.clone(),
            kind: row.kind.clone(),
            version: row.version.clone(),
            locator: row.locator.clone(),
            // The digest is kept: the blob is still in the store, and the row is
            // about the *dropped-in file*, not about the bytes.
            sha256: row.sha256.clone(),
            format: row.format.clone(),
            access_status: row.access_status.clone(),
            route: row.route.clone(),
            access_basis: row.access_basis.clone(),
            label: row.label.clone(),
            caption: row.caption.clone(),
            source_url: row.source_url.clone(),
            publisher: row.publisher.clone(),
            publisher_note: row.publisher_note.clone(),
            imported_from: Some(imported_from.to_string()),
            retrieved_at: row.retrieved_at.clone(),
            missing_on_disk: true,
            // No new bytes to place: `blob: None` means "this row brings no blob
            // row", and the existing one is left exactly as it is.
            blob: None,
        });
    }
    if !flagged_rows.is_empty() {
        db.write_artefacts(&flagged_rows, WriteMode::Reconcile)?;
    }
    Ok(vanished)
}

/// Record the full-text gap a drop-in directory can close.
///
/// Only the full text, for `import_flat::record_gaps`' reason: the directory has
/// a slot named `fulltext.pdf`, so its absence is evidence, while whether a work
/// has SI or tables is not something a directory can tell us.
fn record_gap(
    db: &Database,
    paper_id: &str,
    root: &Path,
    rows: &[ArtefactWrite],
    now: DateTime<Utc>,
) -> Result<Vec<RecordedGap>, ScanError> {
    let holds_fulltext = rows
        .iter()
        .any(|row| row.kind.starts_with("fulltext") && !row.missing_on_disk)
        || db.has_fulltext_artefact(paper_id)?;
    if holds_fulltext {
        // A gap this command recorded is now closed; retract it rather than leave
        // "wanted" sitting beside "have". Scoped by kind and locator rather than
        // by an exact path, because the directory it was recorded against is
        // resolved (`/private/var/…` on macOS) and a string comparison against an
        // unresolved path would silently retract nothing.
        let retracted = db.retract_drop_gaps(paper_id, WANT_FULLTEXT, FULLTEXT_LOCATOR)?;
        if retracted > 0 {
            tracing::debug!(paper_id, retracted, "closed a recorded gap");
        }
        return Ok(Vec::new());
    }

    let drop_path = root.join(FULLTEXT_FILENAME);
    let gap = RecordedGap {
        kind: WANT_FULLTEXT.to_string(),
        locator: FULLTEXT_LOCATOR.to_string(),
        reason: format!(
            "no fulltext.pdf / fulltext.html was found in the reconciled directory {}",
            root.display()
        ),
        drop_path: drop_path.clone(),
    };
    db.upsert_acquisition_state(&StateWrite {
        paper_id: paper_id.to_string(),
        kind: gap.kind.clone(),
        locator: gap.locator.clone(),
        wanted_version: "vor".to_string(),
        status: "pending".to_string(),
        reason: Some(gap.reason.clone()),
        // Both `None`, for `import_flat`'s reason: this command classifies no
        // publisher — it reads files out of a directory and never looks at a
        // DOI — and `action_list` derives the group from the DOI registry at
        // report time regardless (#261).
        publisher: None,
        hint_url: None,
        drop_path: Some(drop_path.to_string_lossy().into_owned()),
        // Closed by a person dropping a file, not by a fetch, so this is not a
        // row in any retry queue.
        next_attempt_at: None,
        updated_at: now.to_rfc3339(),
    })?;
    Ok(vec![gap])
}

/// `attach`'s `kind` argument, resolved against the file's extension.
///
/// A specific full-text kind must agree with the extension, and a bare `fulltext`
/// resolves through it. Anything outside the `artefacts.kind` vocabulary is
/// refused by name rather than defaulted: filing a `.docx` as `fulltext_html`
/// would make `coverage` claim a full text we cannot read.
/// The returned kind is always a `&'static str` — one of `fulltext_kind`'s
/// literals or this function's own — never a slice of `kind`, which is why the
/// non-full-text arm matches the three literals again instead of returning `kind`.
fn resolve_kind(kind: &str, file: &Path) -> Result<&'static str, ScanError> {
    let ext = scitadel_db::sqlite::file_extension(&file.to_string_lossy());
    let invalid = |message: String| ScanError::Adapter(AdapterError::Validation(message));
    match kind {
        "fulltext" | "fulltext_pdf" | "fulltext_html" | "fulltext_xml" => {
            let resolved = fulltext_kind(&ext).ok_or_else(|| {
                invalid(format!(
                    "{ext:?} is not a full-text serialisation, so a full text cannot be recorded \
                     from it. ADR-007 §1 \"Artefact rules\" only counts a `{ext}` file when it \
                     really is the full text, and `artefacts.kind` is a closed vocabulary."
                ))
            })?;
            if kind != "fulltext" && kind != resolved.kind {
                return Err(invalid(format!(
                    "--kind {kind} does not match {ext:?}, which is a `{}`. Attaching it as \
                     {kind} would record a format its bytes do not have (ADR-007 §1 \"Artefact \
                     rules\").",
                    resolved.kind
                )));
            }
            Ok(resolved.kind)
        }
        "si" | "table" | "figure" => Ok(match kind {
            "si" => "si",
            "table" => "table",
            _ => "figure",
        }),
        other => Err(invalid(format!(
            "{other:?} is not an artefacts.kind. Valid: {}",
            ATTACH_KINDS.join(", ")
        ))),
    }
}

/// The drop-in slot an `artefacts.kind` belongs in.
///
/// `resolve_kind` returns one of five literals, so this is total over its output
/// and `unreachable!` is a statement about the two functions being changed
/// together rather than a runtime path.
fn slot_for(kind: &str) -> import_flat::Slot {
    match kind {
        "fulltext_pdf" | "fulltext_html" | "fulltext_xml" => import_flat::Slot::FullText,
        "si" => import_flat::Slot::Si,
        "table" => import_flat::Slot::Table,
        "figure" => import_flat::Slot::Figure,
        other => unreachable!("resolve_kind returned {other:?}"),
    }
}

/// The file `attach` was pointed at, resolved and checked to be a readable file.
///
/// A dangling symlink is refused as `NoSuchFile` naming the path the caller typed,
/// because a person who pointed at a broken link needs to know the link is
/// broken — not that some path below it is missing.
fn resolve_file(file: &Path) -> Result<PathBuf, ScanError> {
    let resolved = import_flat::resolve_best_effort(file);
    if !resolved.exists() {
        return Err(ScanError::NoSuchFile(file.to_path_buf()));
    }
    if !resolved.is_file() {
        return Err(ScanError::Adapter(AdapterError::Validation(format!(
            "{} is not a file",
            resolved.display()
        ))));
    }
    Ok(resolved)
}

/// A byte count in the words a person reading a refusal needs.
fn human_bytes(bytes: i64) -> String {
    match bytes {
        b if b >= 1024 * 1024 * 1024 => format!("{} GB", b / (1024 * 1024 * 1024)),
        b if b >= 1024 * 1024 => format!("{} MB", b / (1024 * 1024)),
        b => format!("{b} bytes"),
    }
}

#[cfg(test)]
mod tests {
    //! The ADR-007 §1 "Manual drop-ins" rules, driven against a real library on
    //! disk with real files — every claim here is about bytes that either got
    //! filed or did not.

    use super::*;
    use scitadel_core::models::PaperId;
    use scitadel_core::ports::PaperRepository as _;

    /// A publisher's "your download is starting" page, which is what 7 of raid's
    /// 27 "SI" files actually were.
    const LANDING_PAGE: &[u8] =
        b"<!DOCTYPE html>\n<html lang=\"en\"><head><title>Preparing your download</title></head>\n\
          <body><p>Your file will be available shortly.</p></body></html>\n";

    /// A real PDF whose `/Title` is the work itself, so the identity check
    /// corroborates rather than objects.
    const SI_FOR_THIS_WORK: &[u8] =
        b"%PDF-1.7\n/Title(Deep learning for radiopharmaceutical image reconstruction)\n\
          body\n%%EOF\n";

    /// A real PDF belonging to a **different** work, saved into this one's
    /// directory.
    const SI_FOR_OTHER_WORK: &[u8] =
        b"%PDF-1.7\n/Title(Total-body PET scanners for theranostics)\nbody\n%%EOF\n";

    /// A supplementary file that really is HTML, and whose Highwire tags name the
    /// work — the shape a publisher's SI page really has.
    const SI_HTML_FOR_THIS_WORK: &[u8] = b"<!DOCTYPE html><html lang=\"en\"><head>\
        <meta name=\"citation_title\" content=\"Deep learning for radiopharmaceutical image reconstruction\">\n        <meta name=\"citation_publication_date\" content=\"2020-04-01\">\n        <meta name=\"citation_author\" content=\"Young, Christopher J.\">\n        </head><body>supplementary tables</body></html>\n";

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
            let work = dir
                .path()
                .join(manifest::MANIFEST_DIR)
                .join(file_stem_for(&paper));
            std::fs::create_dir_all(&work).unwrap();
            Self { dir, db, paper }
        }

        fn work_dir(&self) -> PathBuf {
            self.dir
                .path()
                .join(manifest::MANIFEST_DIR)
                .join(file_stem_for(&self.paper))
        }

        /// Write a file into the work's drop-in tree at a relative path.
        fn drop(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.work_dir().join(rel);
            std::fs::create_dir_all(path.parent().expect("a parent")).unwrap();
            std::fs::write(&path, bytes).unwrap();
            path
        }

        fn scan(&self) -> ScanReport {
            let dir = self.work_dir();
            scan(&self.db, &self.paper, Some(&dir)).expect("scan runs")
        }

        /// `(kind, locator, route, access_status, missing_on_disk, sha256?)`.
        fn artefacts(&self) -> Vec<String> {
            self.db
                .artefacts_for_paper(self.paper.id.as_str())
                .expect("artefacts")
                .into_iter()
                .map(|row| {
                    format!(
                        "{}|{}|{}|{}|{}|{}",
                        row.kind,
                        row.locator,
                        row.route,
                        row.access_status,
                        i64::from(row.missing_on_disk),
                        row.sha256.is_some()
                    )
                })
                .collect()
        }

        /// `(kind, locator, outcome, detail)`.
        fn attempts(&self) -> Vec<(String, String, String, String)> {
            let conn = self.db.conn().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT kind, locator, outcome, COALESCE(detail, '')
                       FROM acquisition_attempts ORDER BY id",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .unwrap();
            rows.map(Result::unwrap).collect()
        }

        fn identity_checks(&self) -> Vec<(String, String, Option<String>, Option<String>)> {
            let conn = self.db.conn().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT phase, status, expected_title, resolved_title
                       FROM paper_identity_checks ORDER BY phase",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                })
                .unwrap();
            rows.map(Result::unwrap).collect()
        }

        fn blob_count(&self) -> i64 {
            self.db
                .conn()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
                .unwrap()
        }
    }

    /// **ADR-007 §1 "Manual drop-ins": "Unknown files get `route = 'manual'`,
    /// `access_status = 'unknown'` and a post-fetch identity check."**
    ///
    /// All three in one test, because they are one sentence: a supplementary file
    /// scitadel did not fetch has no route and no access status to claim, and the
    /// only evidence available about whether it belongs to this work is the bytes.
    /// The identity check's *presence* is the assertion — and it is `unverified`,
    /// not `ok`, because a PDF's `/Title` alone corroborates nothing, which is the
    /// honest verdict and is not a block (see the module docs).
    #[test]
    fn an_unknown_dropped_in_file_gets_route_manual_and_unknown_status() {
        let fx = Fx::new();
        fx.drop("si/Supporting Information S1.pdf", SI_FOR_THIS_WORK);

        let report = fx.scan();

        assert_eq!(
            fx.artefacts(),
            vec!["si|supporting-information-s1|manual|unknown|0|true"],
            "route=manual, access_status=unknown, bytes hashed"
        );
        assert_eq!(report.counts.get("si"), Some(&1));
        assert_eq!(report.blobs, 1, "and the bytes are in the store");
        assert_eq!(fx.blob_count(), 1);

        // The report says the same thing, in the words an operator reads.
        let file = &report.files[0];
        assert_eq!(file.kind, "si");
        assert_eq!(file.locator, "supporting-information-s1");
        assert_eq!(file.refused_because, None);

        // The post-fetch check ran, and recorded what it compared.
        let checks = fx.identity_checks();
        assert_eq!(checks.len(), 1, "{checks:?}");
        assert_eq!(checks[0].0, "post_fetch");
        assert_eq!(
            checks[0].1, "unverified",
            "a PDF title with no year and no author corroborates nothing, and `we could not \
             check` is not `ok` (crate::identity)"
        );
        assert_eq!(
            checks[0].2.as_deref(),
            Some("Deep learning for radiopharmaceutical image reconstruction"),
            "the expectation is the stored work's — a drop-in has no registry behind it"
        );
        assert_eq!(
            checks[0].3.as_deref(),
            Some("Deep learning for radiopharmaceutical image reconstruction"),
            "and both titles are stored, or the row cannot be acted on"
        );
        assert_eq!(report.identity.len(), 1);
        assert_eq!(report.identity[0].status, "unverified");

        // A re-run writes nothing: same rows, same mtime-derived retrieved_at.
        let before = fx.artefacts();
        let again = fx.scan();
        assert_eq!(
            fx.artefacts(),
            before,
            "an unchanged directory writes nothing"
        );
        assert!(again.manifest.is_some(), "but the mirror is regenerated");
    }

    /// **ADR-007 §1: "A file that has disappeared is flagged `missing_on_disk = 1`;
    /// the row stays."**
    ///
    /// Both halves, and the second is the load-bearing one: the row must survive,
    /// because it is the only record that the work *had* this file, where it came
    /// from, and that it is gone — which is what makes `coverage` report the work
    /// as needing it again rather than silently looking complete.
    #[test]
    fn a_vanished_file_is_flagged_missing_and_the_row_stays() {
        let fx = Fx::new();
        let path = fx.drop("si/Supporting Information S1.pdf", SI_FOR_THIS_WORK);

        let first = fx.scan();
        assert_eq!(first.vanished, Vec::<String>::new());
        assert_eq!(
            fx.artefacts(),
            vec!["si|supporting-information-s1|manual|unknown|0|true"],
            "the file is held"
        );

        // It is gone.
        std::fs::remove_file(&path).unwrap();
        assert!(!path.exists(), "precondition: the file has vanished");

        let second = fx.scan();

        assert_eq!(
            second.vanished,
            vec!["si::supporting-information-s1".to_string()],
            "the run names what it lost"
        );
        assert_eq!(
            fx.artefacts(),
            vec!["si|supporting-information-s1|manual|unknown|1|true"],
            "flagged missing_on_disk = 1 — and the row, its locator and its digest all stay"
        );
        // The blob is still in the store: the *dropped-in file* is what vanished,
        // and gc — not scan — is what decides about bytes.
        assert_eq!(fx.blob_count(), 1);

        // And `coverage` believes it: the work is no longer counted as holding it.
        let report = fx.db.coverage_report(None).unwrap();
        let held_si = report
            .by_kind
            .get("si")
            .map(|total| total.held)
            .unwrap_or_default();
        assert_eq!(
            held_si, 0,
            "a `missing_on_disk` row does not close a want (coverage.rs's own clause)"
        );

        // Put it back and the flag clears — the row follows the file.
        fx.drop("si/Supporting Information S1.pdf", SI_FOR_THIS_WORK);
        let third = fx.scan();
        assert_eq!(third.vanished, Vec::<String>::new());
        assert_eq!(
            fx.artefacts(),
            vec!["si|supporting-information-s1|manual|unknown|0|true"]
        );
    }

    /// **ADR-007 §1 "Artefact rules": "SI only counts when its magic bytes match
    /// its declared format. The raid review found 7 of 27 'SI' files on disk were
    /// HTML landing pages."**
    ///
    /// The 7-of-27 case, end to end: an HTML landing page saved as
    /// `Supporting Information S1.pdf`, in the directory a real drop-in goes in.
    /// The claims are all specific because the failure was silent — the file
    /// hashes, it stores, it has a legal extension and a plausible name, so only
    /// the bytes say otherwise.
    #[test]
    fn an_html_landing_page_saved_as_si_is_rejected_by_magic_bytes() {
        let fx = Fx::new();
        fx.drop("si/Supporting Information S1.pdf", LANDING_PAGE);

        let report = fx.scan();

        // Nothing was filed.
        assert!(
            fx.artefacts().is_empty(),
            "a landing page must not become an SI: {:?}",
            fx.artefacts()
        );
        assert_eq!(
            fx.blob_count(),
            0,
            "and no blob either — it never entered the store"
        );
        assert!(
            !self_store_exists(&fx),
            "so there are no bytes on disk under the store for gc to consider later"
        );

        // The report names it, rather than looking like a clean run.
        assert_eq!(report.files.len(), 1);
        assert_eq!(
            report.files[0].refused_because.as_deref(),
            Some("bad_magic"),
            "ADR-007 declares `bad_magic` for exactly this"
        );
        let detail = report.files[0].detail.clone().expect("a reason");
        assert!(detail.contains("declared as pdf"), "{detail}");
        assert!(
            detail.contains("markup document"),
            "the reason says what the bytes are, not only what was claimed: {detail}"
        );

        // `acquisition_attempts.outcome = 'bad_magic'` had no writer in the
        // workspace before this slice; this is it.
        let attempts = fx.attempts();
        assert_eq!(attempts.len(), 1, "{attempts:?}");
        assert_eq!(
            attempts[0].0, "si",
            "recorded against the want, not the file"
        );
        assert_eq!(attempts[0].1, "supporting-information-s1");
        assert_eq!(attempts[0].2, "bad_magic");
        assert_eq!(attempts[0].3, detail);
        assert_eq!(
            fx.db
                .conn()
                .unwrap()
                .query_row(
                    "SELECT route FROM acquisition_attempts WHERE outcome = 'bad_magic'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            ROUTE_MANUAL,
            "a file a person dropped in spends no publisher permit, so no bucket and no route \\
             beyond `manual`"
        );
    }

    /// The other side of the same rule, and the one that keeps it from becoming a
    /// blanket refusal: an HTML supplementary file **that is really HTML** is
    /// filed, and its identity check corroborates from the Highwire tags the page
    /// carries.
    #[test]
    fn an_html_supplementary_file_that_is_really_html_is_filed() {
        let fx = Fx::new();
        fx.drop("si/Supporting Information S1.html", SI_HTML_FOR_THIS_WORK);

        let report = fx.scan();
        assert!(
            report.files.iter().all(|f| f.refused_because.is_none()),
            "declared as html, an HTML page matches: {:?}",
            report.files
        );
        assert_eq!(
            fx.artefacts(),
            vec!["si|supporting-information-s1|manual|unknown|0|true"]
        );
        assert_eq!(
            fx.identity_checks()[0].1,
            "ok",
            "the page's own citation_title, year and author all corroborate the stored work"
        );
        assert_eq!(report.identity[0].status, "ok");
    }

    /// Is the blob store directory absent? A refused file never enters it, which
    /// is what keeps `gc` from ever being asked about the bytes of a landing page.
    fn self_store_exists(fx: &Fx) -> bool {
        fx.dir.path().join(scitadel_db::sqlite::BLOB_DIR).exists()
    }

    /// A file over ADR-007 §1's cap is refused the same way, which is what gives
    /// `too_large` its writer.
    #[test]
    fn a_file_over_its_cap_is_refused_as_too_large() {
        let fx = Fx::new();
        // A figure, so the cap is the small one, without writing 20 MB: sparse.
        let path = fx.drop("figures/fig1.png", b"\x89PNG\r\n\x1a\n");
        let big = (scitadel_db::sqlite::CAP_FIGURE_BYTES + 1) as u64;
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_len(big).unwrap();
        drop(file);

        let report = fx.scan();
        assert_eq!(
            report.files[0].refused_because.as_deref(),
            Some("too_large")
        );
        assert!(
            report.files[0]
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("20 MB cap")),
            "the reason names the cap: {:?}",
            report.files[0].detail
        );
        assert!(fx.artefacts().is_empty(), "and no artefact is created");
        assert_eq!(
            fx.blob_count(),
            0,
            "nor a blob: the cap applies before persistence"
        );
        assert_eq!(fx.attempts()[0].2, "too_large");
    }

    /// A dropped-in file that is another work's is not filed, and saying so is
    /// one `override-identity` away from being filed.
    #[test]
    fn a_dropped_in_file_for_another_work_is_not_filed_until_a_person_says_so() {
        let fx = Fx::new();
        fx.drop("si/Supporting Information S1.pdf", SI_FOR_OTHER_WORK);

        let report = fx.scan();
        assert_eq!(
            report.identity[0].status, "mismatch",
            "the file names another work in its own /Title"
        );
        assert_eq!(
            report.files[0].refused_because.as_deref(),
            Some("identity_mismatch")
        );
        assert!(
            fx.artefacts().is_empty(),
            "#253: nothing is filed under a mismatch"
        );
        assert_eq!(fx.attempts()[0].2, "identity_mismatch");
        assert_eq!(fx.identity_checks()[0].1, "mismatch");

        // A person rules, and the same bytes file this time.
        fx.db
            .override_identity("p-1", "raid confirmed this supplement is for this work")
            .expect("override");
        let after = fx.scan();
        assert_eq!(
            after.identity[0].status, "overridden",
            "the check does not even run once a person has settled the question"
        );
        assert_eq!(
            fx.artefacts(),
            vec!["si|supporting-information-s1|manual|unknown|0|true"],
            "and the file is filed"
        );
    }

    /// `attach --kind` must agree with the file, or the magic-byte check would be
    /// decorative: the command line would be a way in past it.
    #[test]
    fn attach_refuses_a_kind_that_contradicts_the_bytes() {
        let fx = Fx::new();
        let html = fx.drop("dropped.html", LANDING_PAGE);

        let err = attach(&fx.db, &fx.paper, &html, "fulltext_pdf", None, None)
            .expect_err("refused by name");
        let message = err.to_string();
        assert!(message.contains("fulltext_pdf"), "{message}");
        assert!(
            message.contains("html"),
            "and says what the bytes are: {message}"
        );
        assert!(fx.artefacts().is_empty());

        // The honest spelling files it.
        let ok = attach(&fx.db, &fx.paper, &html, "fulltext", None, None).expect("filed");
        assert_eq!(ok.file.refused_because, None);
        assert_eq!(
            fx.artefacts(),
            vec!["fulltext_html||manual|full_text|0|true"],
            "the full-text slot gets `full_text`, because those bytes *are* the full text"
        );
    }

    /// The lease, from the outside: a work another live process holds is not read
    /// and not written — and this is the same `acquisition_leases` upsert the fetch
    /// path uses.
    #[test]
    fn scan_writes_nothing_while_another_live_process_holds_the_work() {
        let fx = Fx::new();
        fx.drop("si/Supporting Information S1.pdf", SI_FOR_THIS_WORK);

        let holder = scitadel_db::sqlite::new_lease_owner();
        fx.db
            .acquire_lease("p-1", &holder, LEASE_TTL_MS)
            .unwrap()
            .expect("claimed");

        let report = fx.scan();
        assert!(report.leased_elsewhere);
        assert!(report.lease.is_none(), "we did not take it");
        assert!(
            report.manifest.is_none(),
            "and wrote no mirror, which is leased work"
        );
        assert!(
            fx.artefacts().is_empty(),
            "and read nothing into the database"
        );
        assert_eq!(fx.blob_count(), 0);
        assert!(
            !fx.dir
                .path()
                .join(manifest::MANIFEST_DIR)
                .join(file_stem_for(&fx.paper))
                .join(manifest::MANIFEST_FILENAME)
                .exists()
        );
    }

    /// A real `scan` writes the mirror by the lease holder, after the commit, and
    /// takes no lease at all when it is a dry run.
    #[test]
    fn a_scan_writes_the_manifest_and_a_plan_does_not() {
        let fx = Fx::new();
        fx.drop("si/Supporting Information S1.pdf", SI_FOR_THIS_WORK);

        let dir = fx.work_dir();
        let plan = plan(&fx.db, &fx.paper, Some(&dir)).expect("planned");
        assert!(plan.manifest.is_none());
        assert!(
            plan.lease.is_none(),
            "a plan takes no lease, so it cannot block a real run"
        );

        let report = fx.scan();
        let mirror = report.manifest.expect("the mirror was written");
        assert!(mirror.written);
        assert_eq!(mirror.owner, report.lease.expect("we held the claim"));
        let path = dir.join(manifest::MANIFEST_FILENAME);
        // Compare canonically on both sides. `mirror.path` comes back
        // canonicalised (macOS resolves its temp dir from `/var` to
        // `/private/var`), while `dir` is whatever the caller passed, so a
        // raw comparison fails on macOS and passes on Linux — which is how a
        // real path-alias bug in this area went unnoticed for one slice.
        // This is the same trap `resolve_best_effort` exists to close.
        assert_eq!(
            import_flat::resolve_best_effort(&mirror.path),
            import_flat::resolve_best_effort(&path),
            "the mirror must land at the path the caller named"
        );
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["work"]["paper_id"], "p-1");
        assert_eq!(parsed["artefacts"].as_array().map(Vec::len), Some(1));
        // No staging residue.
        let residues: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(residues.is_empty(), "{residues:?}");
    }
}
