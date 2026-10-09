//! ADR-007 §1 "Artefact rules": `scitadel gc` — collect unreferenced blobs.
//!
//! > **Unreferenced blobs** are collected by `scitadel gc` (S2).
//!
//! One sentence in the ADR, and it is the only destructive command in the
//! workspace, so the whole module is an argument about safety rather than a
//! walk over a directory.
//!
//! ## The safety argument
//!
//! A blob is the **only** copy of a file the user may have spent an afternoon
//! obtaining. Every rule below exists because a wrong deletion is unrecoverable
//! while a blob that survives is merely wasted disk, so the design is
//! asymmetric on purpose: gc needs positive evidence before it removes
//! anything, and any doubt is a skip.
//!
//! **1. Referencehood is the definition, and it is a foreign key.**
//! A blob is referenced exactly when some `artefacts` row names it. That is not
//! a heuristic scitadel invented — `artefacts.sha256 REFERENCES blobs(sha256)`
//! is migration 013's own constraint, so the store cannot contain a row that
//! claims bytes it has never heard of. The query is therefore a `NOT EXISTS`
//! against `artefacts`, not a join with a second copy of the idea.
//!
//! **2. The plan is re-checked under the write lock, immediately before the
//! delete.** [`collect`] reports from a read, but the delete runs inside one
//! `BEGIN IMMEDIATE` that re-runs the same `NOT EXISTS` guard. So a blob that
//! became referenced between the report a person read and the delete they
//! approved is *not* deleted: the guard is evaluated again after every other
//! writer's transaction has either committed or been made to wait.
//!
//! **3. The grace period is what closes the one window a lock cannot.**
//! [`collect`] refuses any blob younger than [`DEFAULT_MIN_AGE_HOURS`] (24 h by
//! default). This is not superstition, it is the specific residual race:
//!
//! - The store is written in two phases everywhere — file first, rows second
//!   (`download.rs` and `import_flat.rs` both hash and `store_blob` with **no
//!   write lock held**, then take a short transaction).
//! - So a fetch of digest `D` can have its file on disk and its `blobs` row
//!   not yet committed. `gc` only ever considers rows in `blobs`, so it cannot
//!   see that file at all — safe. But the mirror case is not: `store_blob`
//!   short-circuits when the destination exists (content-addressed: same digest,
//!   same bytes), so a fetch of a digest that is *already* in `blobs` writes no
//!   file and only then commits an `artefacts` row. If gc's transaction commits
//!   in between, it removes the row and the file, and the fetch's transaction
//!   — which arrives afterwards and re-`INSERT OR IGNORE`s both rows — leaves an
//!   artefact pointing at bytes that are gone.
//! - No lock closes that, because the file write happens outside the
//!   transaction by design (a 200-page PDF must not be hashed under the write
//!   lock). A 24-hour floor does: any fetch that can still reference a blob is
//!   one that started before gc ran, and 24 h is far longer than a paced
//!   campaign takes over one work. Set `--min-age-hours 0` to collect
//!   everything unreferenced, which is the right choice only with no other
//!   process touching the library.
//!
//! **4. gc never deletes a path it did not derive.** Candidates come from the
//! `blobs` table, and a row is only eligible when its `rel_path` is a blob path
//! *for its own digest*: relative, no `..`, under `blobs/`, inside the
//! two-hex-character shard directory, and named `<digest>[.ext]`. Anything else
//! is reported as skipped. There is no directory walk feeding the delete list
//! and no path taken from an artefact row or an HTTP response, so a hostile
//! `rel_path` cannot aim gc at `/etc` or at a sibling work's files.
//!
//! **5. `blobs/.tmp/` is never a candidate.** It holds staged writes from
//! in-flight fetches, which by rule 3 are exactly the thing gc must not touch,
//! and it is excluded by the path-shape check rather than by a name comparison.
//!
//! **6. Deleting a row and deleting a file are one decision, and a failure is
//! loud.** The row goes first inside the transaction; an unlink that fails
//! leaves an orphaned file, which is the *safe* direction (the next run reports
//! it as an untracked file and the bytes are still there). A rollback would be
//! the unsafe direction, so gc does not pretend to be transactional across the
//! filesystem.
//!
//! ## What gc never does
//!
//! - It never deletes an `artefacts` row. An artefact is a curation fact; a
//!   missing blob is reported as such and left alone.
//! - It never deletes a file that no `blobs` row names. Those are counted and
//!   reported (`untracked_files`) and left on disk — they are somebody's
//!   working files, and gc has no way to know which.
//! - It never follows the legacy `papers/<stem>.<ext>` copies. Those copies
//!   are no longer written (#253's S2e retired the dual-write), so a library
//!   has them only if it predates that change — and they are not inside
//!   `blobs/`, so [`count_untracked_files`] never counted them and never will.
//!   Removing them is a person's decision (`scitadel scan` reconciles them into
//!   artefacts; a human deletes them), not a garbage-collection run's.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::params;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use crate::error::DbError;
use crate::sqlite::Database;
use crate::sqlite::blobs::BLOB_DIR;

/// How old a blob must be before gc will consider it, in hours.
///
/// 24 h, and deliberately not "0": see rule 3 in the module docs. This is the
/// one parameter that trades disk for the risk of destroying the only copy of a
/// file, and it is exposed as a flag so a person can widen it, never silently
/// narrowed.
pub const DEFAULT_MIN_AGE_HOURS: i64 = 24;

/// A blob row, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlobRow {
    pub sha256: String,
    pub bytes: i64,
    pub mime: Option<String>,
    /// Root-relative (ADR-007 §1 "Storage"): today's absolute paths break as
    /// soon as a library moves.
    pub rel_path: String,
    /// RFC 3339 UTC.
    pub created_at: String,
}

/// One blob gc found unreferenced and old enough to collect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GcCandidate {
    pub sha256: String,
    pub rel_path: String,
    pub bytes: i64,
    /// Absolute path the file occupies under this library's root.
    pub absolute_path: PathBuf,
    /// RFC 3339 UTC — when the bytes arrived, which is the clock the grace
    /// period reads.
    pub created_at: String,
}

/// An unreferenced blob gc would **not** collect, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GcSkip {
    pub sha256: String,
    pub rel_path: String,
    /// Machine-readable enough to be asserted on, prose enough to print.
    pub reason: String,
}

/// What one `gc` run found and did.
///
/// Under `--dry-run` every field describes what **would** happen, and
/// `collected_rows` / `removed_files` are both zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub min_age_hours: i64,
    /// Rows in `blobs`.
    pub blobs_total: usize,
    /// Of those, rows some `artefacts` row names.
    pub referenced_total: usize,
    /// Collectable now: unreferenced, old enough, and at a path gc is willing
    /// to remove.
    pub candidates: Vec<GcCandidate>,
    /// Unreferenced but not collectable — too new, or at a path gc refuses to
    /// touch.
    pub skipped: Vec<GcSkip>,
    /// `blobs` rows deleted. Always zero for a dry run.
    pub collected_rows: usize,
    /// Files unlinked. Always zero for a dry run.
    pub removed_files: usize,
    /// `blobs` rows that were collectable but had no file on disk.
    pub missing_files: usize,
    /// Files under `blobs/` that no `blobs` row names. Counted, never touched
    /// — see the module docs.
    pub untracked_files: usize,
    /// Bytes the collected rows described. What `--dry-run` would reclaim.
    pub reclaimable_bytes: i64,
}

impl GcReport {
    /// Bytes actually reclaimed by a real run; zero for a dry run.
    #[must_use]
    pub fn freed_bytes(&self) -> i64 {
        if self.dry_run {
            0
        } else {
            self.reclaimable_bytes
        }
    }
}

/// Anything that can stop a gc run. All of them are refusals: gc has no
/// partial-success mode, because "collected some blobs" is indistinguishable
/// from "collected the wrong blobs" to whoever was holding the only copy.
#[derive(Debug, thiserror::Error)]
pub enum GcError {
    #[error(
        "gc needs a file-backed library: an in-memory database has no root, so it holds no \
         blobs to collect (ADR-007 §1 \"Storage\")"
    )]
    NoLibraryRoot,
    #[error(transparent)]
    Db(#[from] DbError),
    /// A raw `rusqlite` failure inside the collection transaction. Kept
    /// distinct from [`GcError::Db`] so the message names the stage.
    #[error("sqlite failed while collecting blobs: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Every `blobs` row, newest first.
pub fn all_blobs(conn: &Connection) -> Result<Vec<BlobRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT sha256, bytes, mime, rel_path, created_at FROM blobs
         ORDER BY created_at DESC, sha256",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(BlobRow {
            sha256: row.get(0)?,
            bytes: row.get(1)?,
            mime: row.get(2)?,
            rel_path: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// The digests some `artefacts` row names.
///
/// The same `NOT EXISTS` shape [`collect`] deletes under, exposed on its own so
/// a test (and a reader) can ask the question the ADR's sentence asks without
/// running a collection.
pub fn referenced_digests(conn: &Connection) -> Result<std::collections::HashSet<String>, DbError> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT sha256 FROM artefacts WHERE sha256 IS NOT NULL")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = std::collections::HashSet::new();
    for row in rows {
        out.insert(row?);
    }
    Ok(out)
}

/// Find the collectable blobs, without deleting anything.
///
/// `now` is passed in for the same reason leases take it: the grace period is
/// measured against one explicitly supplied instant, so the rule is testable
/// without sleeping.
pub fn plan(
    conn: &Connection,
    library_root: &Path,
    min_age_hours: i64,
    now: DateTime<Utc>,
) -> Result<(Vec<GcCandidate>, Vec<GcSkip>), DbError> {
    let cutoff = now - chrono::Duration::hours(min_age_hours.max(0));
    let mut stmt = conn.prepare(
        "SELECT sha256, bytes, rel_path, created_at
           FROM blobs
          WHERE NOT EXISTS (SELECT 1 FROM artefacts WHERE artefacts.sha256 = blobs.sha256)
          ORDER BY created_at DESC, sha256",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    for row in rows {
        let (sha256, bytes, rel_path, created_at) = row?;
        let skip = |reason: String| GcSkip {
            sha256: sha256.clone(),
            rel_path: rel_path.clone(),
            reason,
        };
        if let Err(reason) = blob_path_shape(&sha256, &rel_path) {
            skipped.push(skip(reason));
            continue;
        }
        // An unparseable `created_at` is not evidence of age, and "not evidence
        // of age" is the direction gc refuses to guess in.
        match parse(&created_at) {
            None => skipped.push(skip(format!(
                "created_at {created_at:?} is not an RFC 3339 timestamp, so the file's age is \
                 unknown; a file of unknown age is not collected"
            ))),
            Some(created) if created > cutoff => skipped.push(skip(format!(
                "only {:.1}h old; gc collects nothing younger than {min_age_hours}h \
                 (an in-flight fetch may still be about to reference these bytes)",
                (now - created).num_seconds() as f64 / 3600.0
            ))),
            Some(_) => candidates.push(GcCandidate {
                sha256,
                rel_path: rel_path.clone(),
                bytes,
                absolute_path: library_root.join(&rel_path),
                created_at,
            }),
        }
    }
    Ok((candidates, skipped))
}

/// Collect unreferenced blobs, or report what would be collected.
///
/// The plan is a read ([`plan`]); the delete is a separate `BEGIN IMMEDIATE`
/// that re-checks every candidate's referencehood under the write lock before
/// removing it — see rule 2 in the module docs.
///
/// Files that no `blobs` row names are counted into
/// [`GcReport::untracked_files`] and left alone.
pub fn collect(
    conn: &mut Connection,
    library_root: &Path,
    dry_run: bool,
    min_age_hours: i64,
    now: DateTime<Utc>,
) -> Result<GcReport, GcError> {
    let blobs_total: usize = conn.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))?;
    let referenced: std::collections::HashSet<String> = referenced_digests(conn)?;
    let referenced_total = referenced.len();
    let (candidates, mut skipped) = plan(conn, library_root, min_age_hours, now)?;
    let reclaimable_bytes = candidates.iter().map(|c| c.bytes).sum();

    let mut collected_rows = 0usize;
    let mut removed_files = 0usize;
    let mut missing_files = 0usize;

    if !dry_run && !candidates.is_empty() {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for candidate in &candidates {
            // Rule 2: re-checked here, under the write lock, after every other
            // writer's transaction has committed or been made to wait.
            let still_referenced: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM artefacts WHERE sha256 = ?1)",
                params![candidate.sha256],
                |r| r.get(0),
            )?;
            if still_referenced {
                skipped.push(GcSkip {
                    sha256: candidate.sha256.clone(),
                    rel_path: candidate.rel_path.clone(),
                    reason: "became referenced while gc was running; left in place".to_string(),
                });
                continue;
            }
            collected_rows += tx.execute(
                "DELETE FROM blobs WHERE sha256 = ?1 AND NOT EXISTS (
                     SELECT 1 FROM artefacts WHERE artefacts.sha256 = blobs.sha256)",
                params![candidate.sha256],
            )?;

            // The file goes after the row, inside the same transaction: an
            // unlink that fails leaves an orphan file, which is the safe
            // direction (rule 6).
            match std::fs::remove_file(&candidate.absolute_path) {
                Ok(()) => removed_files += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing_files += 1,
                Err(e) => {
                    // Loud, and the row stays deleted: the bytes are the
                    // unreferenced ones either way, and a silent orphan is how
                    // a store grows without anyone noticing.
                    tracing::warn!(
                        path = %candidate.absolute_path.display(),
                        error = %e,
                        "gc removed the blob row but could not unlink the file"
                    );
                    missing_files += 1;
                }
            }
        }
        tx.commit()?;
    }

    Ok(GcReport {
        dry_run,
        min_age_hours: min_age_hours.max(0),
        blobs_total,
        referenced_total,
        candidates,
        skipped,
        collected_rows,
        removed_files,
        missing_files,
        untracked_files: count_untracked_files(library_root)?,
        reclaimable_bytes,
    })
}

/// Files under the blob store that no `blobs` row names. Counted, never touched.
///
/// Two kinds, both of which gc refuses to delete and both of which are worth
/// reporting: staged writes from in-flight fetches (`blobs/.tmp/…`, whose
/// digests are in no table yet) and files somebody put in the store by hand.
/// The count is what makes "gc collected nothing" and "gc did not look" two
/// different messages.
fn count_untracked_files(library_root: &Path) -> Result<usize, DbError> {
    let store = library_root.join(BLOB_DIR);
    let mut count = 0usize;
    let Ok(shards) = std::fs::read_dir(&store) else {
        return Ok(0);
    };
    for shard in shards.flatten() {
        if !shard.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(shard.path()) else {
            continue;
        };
        for file in files.flatten() {
            if file.path().is_file() {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// Is `rel_path` a blob path that gc is willing to delete?
///
/// Rule 4. A pure function of the two strings, so it can be tested exhaustively
/// without a file system — and the checks are *shape* checks, not existence
/// checks, which is what makes them able to refuse a hostile path before it is
/// ever joined.
fn blob_path_shape(sha256: &str, rel_path: &str) -> Result<(), String> {
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(format!(
            "{sha256:?} is not a sha256 hex digest, so its blob path cannot be derived"
        ));
    }
    let path = Path::new(rel_path);
    if path.is_absolute() {
        return Err(format!(
            "{rel_path} is absolute; stored paths are relative to the library root"
        ));
    }
    if rel_path.contains('\0') {
        return Err(format!("{rel_path} contains a NUL byte"));
    }
    let components: Vec<_> = path.components().collect();
    if components.iter().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(format!(
            "{rel_path} contains a `..` or `.` component, so it is not a normalised store path"
        ));
    }
    let [dir, shard, file] = components.as_slice() else {
        return Err(format!("{rel_path} is not `blobs/<2 hex>/<digest>[.ext]`"));
    };
    if dir.as_os_str() != std::ffi::OsStr::new(BLOB_DIR) {
        return Err(format!("{rel_path} is not under `blobs/`"));
    }
    if shard.as_os_str() != std::ffi::OsStr::new(&sha256[..2]) {
        return Err(format!(
            "{rel_path} is not in the shard directory its own digest names ({})",
            &sha256[..2]
        ));
    }
    let file = file.as_os_str().to_string_lossy();
    // `<digest>` or `<digest>.<ext>`, and nothing else. The extension is
    // restricted to alphanumerics so a name can never carry a separator.
    let named_after_its_own_digest = match file.split_once('.') {
        None => file == sha256,
        Some((stem, ext)) => {
            stem == sha256 && !ext.is_empty() && ext.bytes().all(|b| b.is_ascii_alphanumeric())
        }
    };
    if !named_after_its_own_digest {
        return Err(format!(
            "{rel_path} is not named after its own digest, so gc will not remove it"
        ));
    }
    Ok(())
}

/// Parse an RFC 3339 timestamp, `None` when it is not one.
fn parse(stamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(stamp)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

impl Database {
    /// ADR-007 §1 "Artefact rules": report or collect unreferenced blobs. See
    /// the module docs for the safety argument.
    ///
    /// # Errors
    ///
    /// [`GcError::NoLibraryRoot`] for an in-memory database — which holds no
    /// blobs at all, so there is nothing to collect and a silent success would
    /// be a lie — and any SQLite failure.
    pub fn collect_blobs(&self, dry_run: bool, min_age_hours: i64) -> Result<GcReport, GcError> {
        let library_root = self.library_root()?.ok_or(GcError::NoLibraryRoot)?;
        let mut conn = self.pool.get().map_err(DbError::from)?;
        collect(&mut conn, &library_root, dry_run, min_age_hours, Utc::now())
    }

    /// Every blob row, for a report that has to enumerate the store.
    pub fn blobs(&self) -> Result<Vec<BlobRow>, DbError> {
        let conn = self.pool.get()?;
        all_blobs(&conn)
    }

    /// Every work currently leased, so a report can name what another process
    /// is working on. See [`crate::sqlite::leases`].
    pub fn leases(&self) -> Result<Vec<crate::sqlite::LeaseRow>, DbError> {
        let conn = self.pool.get()?;
        crate::sqlite::leases::all_leases(&conn)
    }

    /// Claim a work, renewing while we work on it. See
    /// [`crate::sqlite::leases::acquire`].
    pub fn acquire_lease(
        &self,
        paper_id: &str,
        owner: &str,
        ttl_ms: i64,
    ) -> Result<Option<String>, DbError> {
        let conn = self.pool.get()?;
        crate::sqlite::leases::acquire(
            &conn,
            paper_id,
            owner,
            crate::sqlite::leases::unix_millis(),
            ttl_ms,
        )
    }

    /// Extend a claim we still hold. `false` means we lost the work.
    pub fn renew_lease(&self, paper_id: &str, owner: &str, ttl_ms: i64) -> Result<bool, DbError> {
        let conn = self.pool.get()?;
        crate::sqlite::leases::renew(
            &conn,
            paper_id,
            owner,
            crate::sqlite::leases::unix_millis(),
            ttl_ms,
        )
    }

    /// Give a claim up, if we still hold it.
    pub fn release_lease(&self, paper_id: &str, owner: &str) -> Result<bool, DbError> {
        let conn = self.pool.get()?;
        crate::sqlite::leases::release(&conn, paper_id, owner)
    }

    /// The lease on `paper_id`, if any. The manifest writer's ownership check.
    pub fn lease_holder(&self, paper_id: &str) -> Result<Option<crate::sqlite::LeaseRow>, DbError> {
        let conn = self.pool.get()?;
        crate::sqlite::leases::holder(&conn, paper_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::blobs::{BLOB_TMP_DIR, blob_rel_path, store_blob};
    use crate::sqlite::{ACCESS_BASIS_MANUAL, ArtefactWrite, VERSION_UNKNOWN};

    struct Fx {
        dir: tempfile::TempDir,
        db: crate::sqlite::Database,
        root: PathBuf,
    }

    impl Fx {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = crate::sqlite::Database::open(&dir.path().join("scitadel.db")).unwrap();
            db.migrate().unwrap();
            let conn = db.conn().unwrap();
            conn.execute(
                "INSERT INTO papers (id, title, authors, created_at, updated_at)
                 VALUES ('p-1', 't', '[]', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
                [],
            )
            .unwrap();
            drop(conn);
            let root = dir.path().to_path_buf();
            Self { dir, db, root }
        }

        /// Write `body` into the store the way every writer does, and record the
        /// `blobs` row with a chosen `created_at` so the grace period is
        /// testable without sleeping.
        fn put_blob(&self, body: &[u8], created_at: &str) -> String {
            let src = self.dir.path().join(format!("staged-{}", body.len()));
            std::fs::write(&src, body).unwrap();
            let (sha, bytes) = crate::sqlite::blobs::hash_file(&src).unwrap();
            store_blob(&src, &sha, "pdf", &self.root).unwrap();
            let conn = self.db.conn().unwrap();
            conn.execute(
                "INSERT INTO blobs (sha256, bytes, mime, rel_path, created_at)
                 VALUES (?1, ?2, 'application/pdf', ?3, ?4)",
                params![sha, bytes, blob_rel_path(&sha, "pdf"), created_at],
            )
            .unwrap();
            sha
        }

        /// An `artefacts` row naming `sha` — the reference that makes a blob
        /// live.
        fn reference(&self, sha: &str) {
            let mut conn = self.db.conn().unwrap();
            crate::sqlite::artefacts::write_artefacts(
                &mut conn,
                &[ArtefactWrite {
                    id: String::new(),
                    paper_id: "p-1".into(),
                    kind: "fulltext_pdf".into(),
                    version: VERSION_UNKNOWN.into(),
                    locator: String::new(),
                    sha256: Some(sha.into()),
                    format: Some("pdf".into()),
                    access_status: "full_text".into(),
                    route: "manual".into(),
                    access_basis: ACCESS_BASIS_MANUAL.into(),
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
                    blob: None,
                }],
                crate::sqlite::WriteMode::Reconcile,
            )
            .unwrap();
        }

        fn collect(&self, dry_run: bool, min_age_hours: i64) -> GcReport {
            let mut conn = self.db.conn().unwrap();
            collect(&mut conn, &self.root, dry_run, min_age_hours, Utc::now()).unwrap()
        }

        fn blob_files(&self) -> Vec<PathBuf> {
            let store = self.root.join(BLOB_DIR);
            let mut out = Vec::new();
            if let Ok(shards) = std::fs::read_dir(&store) {
                for shard in shards.flatten() {
                    if shard.file_name() == std::ffi::OsStr::new(BLOB_TMP_DIR) {
                        continue;
                    }
                    if let Ok(files) = std::fs::read_dir(shard.path()) {
                        out.extend(files.flatten().map(|f| f.path()));
                    }
                }
            }
            out.sort();
            out
        }
    }

    const OLD: &str = "2020-01-01T00:00:00+00:00";

    /// An hour ago — inside the grace period, and in the past, so the same row
    /// becomes collectable the moment the floor is lowered.
    fn recent() -> String {
        (Utc::now() - chrono::Duration::hours(1)).to_rfc3339()
    }

    /// The criterion, asserted on the file system and not on a row count: a
    /// blob some artefact names is still there afterwards.
    ///
    /// Three blobs, one referenced, all past the grace period — and the
    /// reported candidate list is asserted too, because "gc deleted nothing" is
    /// also what a `plan` that found nothing would produce.
    #[test]
    fn gc_never_deletes_a_referenced_blob() {
        let fx = Fx::new();
        let held = fx.put_blob(b"%PDF-1.7 held\n%%EOF\n", OLD);
        let garbage = fx.put_blob(b"%PDF-1.7 orphan\n%%EOF\n", OLD);
        fx.reference(&held);

        let before = fx.blob_files();
        assert_eq!(before.len(), 2, "both blobs are on disk to begin with");

        let report = fx.collect(false, 0);

        assert_eq!(
            report
                .candidates
                .iter()
                .map(|c| c.sha256.as_str())
                .collect::<Vec<_>>(),
            vec![garbage.as_str()],
            "only the unreferenced blob is a candidate"
        );
        assert_eq!(report.referenced_total, 1);
        assert_eq!(report.collected_rows, 1);
        assert_eq!(report.removed_files, 1);

        let after = fx.blob_files();
        assert_eq!(after.len(), 1, "exactly one file was removed");
        assert!(
            after.iter().any(|p| p.to_string_lossy().contains(&held)),
            "the referenced blob's bytes must survive: {after:?}"
        );
        assert!(
            fx.root.join(blob_rel_path(&held, "pdf")).exists(),
            "and it must be at the path the artefacts row names"
        );

        // And the reference itself is untouched: gc never deletes an artefact.
        let conn = fx.db.conn().unwrap();
        assert_eq!(
            referenced_digests(&conn)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![&held]
        );
        let blobs = all_blobs(&conn).unwrap();
        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs[0].sha256, held);
    }

    /// `--dry-run` reports exactly what a real run would do and removes nothing:
    /// no row, no file, no bytes.
    #[test]
    fn gc_dry_run_deletes_nothing() {
        let fx = Fx::new();
        let garbage = fx.put_blob(b"%PDF-1.7 orphan\n%%EOF\n", OLD);
        let before = fx.blob_files();

        let report = fx.collect(true, 0);

        assert!(report.dry_run);
        assert_eq!(
            report
                .candidates
                .iter()
                .map(|c| c.sha256.as_str())
                .collect::<Vec<_>>(),
            vec![garbage.as_str()],
            "a dry run names what it would collect"
        );
        assert!(report.reclaimable_bytes > 0, "and what it would reclaim");
        assert_eq!(report.collected_rows, 0);
        assert_eq!(report.removed_files, 0);
        assert_eq!(report.freed_bytes(), 0, "a dry run reclaims nothing");
        assert_eq!(fx.blob_files(), before, "no file was touched");
        let conn = fx.db.conn().unwrap();
        assert_eq!(all_blobs(&conn).unwrap().len(), 1, "no row was touched");
        assert!(
            fx.root.join(blob_rel_path(&garbage, "pdf")).exists(),
            "the file is still on disk"
        );
    }

    /// Rule 3, pinned: the grace period is what keeps an in-flight fetch's
    /// bytes alive, so a blob that was stored a moment ago is reported as a
    /// skip and not collected.
    #[test]
    fn a_blob_younger_than_the_grace_period_is_skipped() {
        let fx = Fx::new();
        let fresh = fx.put_blob(b"%PDF-1.7 in flight\n%%EOF\n", &recent());

        let report = fx.collect(false, DEFAULT_MIN_AGE_HOURS);
        assert!(report.candidates.is_empty(), "{:?}", report.candidates);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].sha256, fresh);
        assert!(
            report.skipped[0].reason.contains("in-flight fetch"),
            "the skip says why: {}",
            report.skipped[0].reason
        );
        assert_eq!(report.collected_rows, 0);
        assert!(fx.root.join(blob_rel_path(&fresh, "pdf")).exists());

        // The same blob, once old enough, is collectable.
        let aged = fx.collect(false, 0);
        assert_eq!(aged.candidates.len(), 1);
        assert_eq!(aged.removed_files, 1);
    }

    /// Rule 5: a staged write in `blobs/.tmp/` is never a candidate and never
    /// counted as collectable, however unreferenced it is.
    #[test]
    fn a_staged_write_is_never_collected() {
        let fx = Fx::new();
        let staging = fx.root.join(BLOB_DIR).join(BLOB_TMP_DIR);
        std::fs::create_dir_all(&staging).unwrap();
        let staged = staging.join("abcdef.1234.tmp");
        std::fs::write(&staged, b"%PDF-1.7 mid-write\n%%EOF\n").unwrap();
        fx.put_blob(b"%PDF-1.7 orphan\n%%EOF\n", OLD);

        let report = fx.collect(false, 0);
        assert_eq!(report.candidates.len(), 1, "only the committed blob");
        assert!(staged.exists(), "the staged write is untouched");
        assert_eq!(report.untracked_files, 1, "and it is reported, not deleted");
    }

    /// Rule 4: a `rel_path` that is not a blob path for its own digest is never
    /// followed, whatever it points at. Exhaustive over the shapes that matter,
    /// because it is the check that stands between a hostile row and a deleted
    /// file outside the store.
    #[test]
    fn only_a_digests_own_blob_path_is_willng_to_be_deleted() {
        let sha = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        assert!(blob_path_shape(sha, &blob_rel_path(sha, "pdf")).is_ok());
        assert!(blob_path_shape(sha, &blob_rel_path(sha, "")).is_ok());
        for hostile in [
            "/etc/passwd".to_string(),
            "../../etc/passwd".to_string(),
            "blobs/ab/../ab/x.pdf".to_string(),
            "papers/other/fulltext.pdf".to_string(),
            "blobs/cd/other.pdf".to_string(),
            "blobs/ab/not-the-digest.pdf".to_string(),
            "blobs/.tmp/abcdef.pdf".to_string(),
            "blobs/ab".to_string(),
            format!("blobs/ab/{sha}.pdf/extra"),
        ] {
            assert!(
                blob_path_shape(sha, &hostile).is_err(),
                "{hostile:?} must never be deletable"
            );
        }
        // A digest that is not a sha256 cannot have a blob path at all.
        assert!(blob_path_shape("not-a-digest", "blobs/no/x.pdf").is_err());
        assert!(blob_path_shape(&sha.to_uppercase(), &blob_rel_path(sha, "pdf")).is_err());
    }

    /// An artefact with a NULL `sha256` (a figure reference with no file) must
    /// not make the store's other blobs look referenced, and must not make the
    /// query fail.
    #[test]
    fn a_figure_reference_counts_for_nothing() {
        let fx = Fx::new();
        let garbage = fx.put_blob(b"%PDF-1.7 orphan\n%%EOF\n", OLD);
        let mut conn = fx.db.conn().unwrap();
        crate::sqlite::artefacts::write_artefacts(
            &mut conn,
            &[ArtefactWrite {
                id: String::new(),
                paper_id: "p-1".into(),
                kind: "figure".into(),
                version: VERSION_UNKNOWN.into(),
                locator: "fig-1".into(),
                sha256: None,
                format: None,
                access_status: "unknown".into(),
                route: "manual".into(),
                access_basis: ACCESS_BASIS_MANUAL.into(),
                label: Some("Figure 1".into()),
                caption: Some("a binding curve".into()),
                source_url: Some("https://example.org/fig1.png".into()),
                publisher: None,
                publisher_note: None,
                license_url: None,
                license_content_version: None,
                license_start: None,
                license_source: None,
                imported_from: None,
                retrieved_at: "2026-01-01T00:00:00+00:00".into(),
                missing_on_disk: false,
                blob: None,
            }],
            crate::sqlite::WriteMode::Reconcile,
        )
        .unwrap();
        assert!(referenced_digests(&conn).unwrap().is_empty());
        drop(conn);

        let report = fx.collect(false, 0);
        assert_eq!(
            report
                .candidates
                .iter()
                .map(|c| c.sha256.as_str())
                .collect::<Vec<_>>(),
            vec![garbage.as_str()]
        );
    }
}
