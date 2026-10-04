//! ADR-007 §1 "Storage": the content-addressed blob store.
//!
//! Blobs live at `<library_root>/blobs/<first 2 hex of sha256>/<sha256>.<ext>`
//! with `rel_path` relative to the library root (the absolute parent of
//! the DB file). A download or an import is staged in `blobs/.tmp/` —
//! same filesystem, so the rename into place is atomic — and skipped
//! entirely when the destination already exists, because the same digest
//! is by definition the same bytes.
//!
//! These primitives are shared by every writer of `blobs`: the legacy
//! backfill (`sqlite::acquisition`) and the flat-layout importer
//! (`scitadel_adapters::import_flat`). There is exactly one
//! implementation, so the same bytes can never get two different store
//! paths or two different artefact ids.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::error::DbError;

/// The store directory, relative to the library root.
pub const BLOB_DIR: &str = "blobs";

/// The staging directory inside [`BLOB_DIR`] — same filesystem as the
/// store itself, which is what makes the final `rename` atomic.
pub const BLOB_TMP_DIR: &str = ".tmp";

/// ADR-007 §1 "Storage", the size-cap table: a file over its cap is recorded
/// as an attempt with outcome `too_large`, and no artefact is created.
pub const CAP_FULLTEXT_BYTES: i64 = 100 * 1024 * 1024;
/// A single supplementary file. The largest of the three, because SI is
/// unbounded in kind and routinely a 180-page PDF or a full supplementary
/// workbook.
pub const CAP_SI_BYTES: i64 = 250 * 1024 * 1024;
/// One figure or table image.
pub const CAP_FIGURE_BYTES: i64 = 20 * 1024 * 1024;

/// The cap that applies to an `artefacts.kind`, in bytes.
///
/// `None` for a kind with no cap in ADR-007 §1's table. Deliberately a
/// **function of the artefact kind and not of its format**, so a 300 MB
/// supplementary workbook cannot be filed as a `figure` to get under the
/// smaller cap.
///
/// The `fulltext_*` kinds share one cap; `table` shares the figure cap, because
/// a table artefact is a published image of a table and nothing in the ADR's
/// table promises otherwise.
#[must_use]
pub fn cap_for_kind(kind: &str) -> Option<i64> {
    Some(match kind {
        "fulltext_pdf" | "fulltext_html" | "fulltext_xml" => CAP_FULLTEXT_BYTES,
        "si" => CAP_SI_BYTES,
        "figure" | "table" => CAP_FIGURE_BYTES,
        _ => return None,
    })
}

/// The library root: the absolute parent directory of the DB file.
/// `None` for an in-memory database, whose `PRAGMA database_list` file
/// is the empty string — such a DB gets no blobs (ADR-007 §1
/// "Storage").
pub fn library_root(conn: &Connection) -> Result<Option<PathBuf>, DbError> {
    let file: Option<String> = conn
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .ok();
    let Some(file) = file.filter(|f| !f.is_empty()) else {
        return Ok(None);
    };
    let absolute = std::fs::canonicalize(&file).unwrap_or_else(|_| PathBuf::from(&file));
    Ok(absolute.parent().map(Path::to_path_buf))
}

/// Where a blob lives, relative to the library root — ADR-007 §1
/// "Storage": `<root>/blobs/<first 2 hex>/<sha256>.<ext>`.
#[must_use]
pub fn blob_rel_path(sha256: &str, ext: &str) -> String {
    if ext.is_empty() {
        format!("{BLOB_DIR}/{}/{sha256}", &sha256[..2])
    } else {
        format!("{BLOB_DIR}/{}/{sha256}.{ext}", &sha256[..2])
    }
}

/// SHA-256 (hex) and byte length of a file, read in chunks so a large
/// PDF never has to fit in memory.
pub fn hash_file(path: &Path) -> Result<(String, i64), DbError> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| io_error("open", path, &e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    let mut bytes = 0_i64;
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| io_error("read", path, &e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        bytes += read as i64;
    }
    Ok((hex_digest(hasher.finalize().as_slice()), bytes))
}

/// Copy `src` into `<library_root>/blobs/<first 2 hex>/<sha256>.<ext>`.
/// The copy is staged in `blobs/.tmp/` and renamed into place, so a
/// reader never sees a half-written blob and a crash never leaves one.
pub fn store_blob(src: &Path, sha256: &str, ext: &str, library_root: &Path) -> Result<(), DbError> {
    let dest = library_root.join(blob_rel_path(sha256, ext));
    if dest.exists() {
        // Content-addressed: same digest, same bytes.
        return Ok(());
    }

    let store = library_root.join(BLOB_DIR);
    let staging = store
        .join(BLOB_TMP_DIR)
        .join(format!("{sha256}.{}.tmp", std::process::id()));
    if let Some(dir) = staging.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_error("create blob tmp dir", dir, &e))?;
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_error("create blob dir", dir, &e))?;
    }
    std::fs::copy(src, &staging).map_err(|e| io_error("copy file into blob store", src, &e))?;

    match std::fs::rename(&staging, &dest) {
        Ok(()) => Ok(()),
        // Another process may have won the race with identical bytes.
        Err(_) if dest.exists() => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&staging);
            Err(io_error("place blob in store", &dest, &e))
        }
    }
}

/// The lowercased extension of `path`, without the dot. Non-alphanumeric
/// bytes are dropped, so a hostile file name can never escape the blob
/// store directory through the name its content is stored under.
#[must_use]
pub fn file_extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|ext| {
            ext.to_string_lossy()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_lowercase()
        })
        .unwrap_or_default()
}

/// Lowercase hex of `bytes`.
#[must_use]
pub fn hex_digest(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// File-system trouble as a `DbError::Migration`, naming the path that
/// caused it. Shared so every store failure reads the same way.
pub fn io_error(what: &str, path: &Path, e: &std::io::Error) -> DbError {
    DbError::Migration(format!("{what} {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_rel_path_is_root_relative_and_sharded_by_digest_prefix() {
        let sha = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        assert_eq!(blob_rel_path(sha, "pdf"), format!("blobs/ab/{sha}.pdf"));
        assert_eq!(blob_rel_path(sha, ""), format!("blobs/ab/{sha}"));
        assert!(!blob_rel_path(sha, "pdf").starts_with('/'));
    }

    /// The extension is the only part of a file name that reaches the
    /// store path, so a hostile one cannot climb out of `blobs/`.
    #[test]
    fn file_extension_drops_anything_that_is_not_alphanumeric() {
        assert_eq!(file_extension("fulltext.pdf"), "pdf");
        assert_eq!(file_extension("a/b/TABLE-1.CSV"), "csv");
        // A space in the extension is dropped rather than escaping as a
        // path separator.
        assert_eq!(file_extension("fulltext.p df"), "pdf");
        // Only the last component's extension is ever used, so directory
        // components cannot reach the store path at all.
        assert_eq!(file_extension("../../etc/passwd.pdf"), "pdf");
        assert_eq!(file_extension("no-extension"), "");
        assert_eq!(file_extension("trailing."), "");
    }

    /// ADR-007 §1 "Storage"'s table, pinned so a later edit to one cap cannot
    /// quietly change another. The caps are policy, and policy that moves
    /// without a test is policy nobody reviewed.
    #[test]
    fn the_size_caps_are_the_adrs_table() {
        assert_eq!(cap_for_kind("fulltext_pdf"), Some(100 * 1024 * 1024));
        assert_eq!(cap_for_kind("fulltext_html"), Some(100 * 1024 * 1024));
        assert_eq!(cap_for_kind("fulltext_xml"), Some(100 * 1024 * 1024));
        assert_eq!(cap_for_kind("si"), Some(250 * 1024 * 1024));
        assert_eq!(cap_for_kind("figure"), Some(20 * 1024 * 1024));
        // SI is the unbounded kind, so it gets the largest cap by a wide
        // margin — and no other kind borrows it.
        assert!(cap_for_kind("si") > cap_for_kind("figure"));
        assert_eq!(cap_for_kind("nonsense"), None);
    }

    #[test]
    fn hash_file_reports_the_digest_and_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.pdf");
        let body = b"%PDF-1.7 body";
        std::fs::write(&path, body).unwrap();

        let mut expected = Sha256::new();
        expected.update(body);
        let (sha, bytes) = hash_file(&path).unwrap();
        assert_eq!(sha, hex_digest(&expected.finalize()));
        assert_eq!(bytes, body.len() as i64);
        assert!(hash_file(&dir.path().join("gone.pdf")).is_err());
    }
}
