//! `.scitadel-bib.lock` sidecar for `bib snapshot` / `bib verify` (#178).
//!
//! Carry-over of the dropped pieces of #132. The lockfile is the
//! comparison anchor that lets `bib verify` detect drift (shortlist or
//! content changed) versus stale (algorithm or scitadel version moved
//! since the lockfile was written).
//!
//! Schema is intentionally extensible via the `format` discriminant so
//! the CSL-JSON variant in #135 can layer on without a schema bump.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Algorithm-pinned `algo_hash` constant for BibTeX exports — sourced
/// from the same `KEY_ALGO_HASH` ADR-006 freezes in [`crate::bibtex`].
/// Sidecar code references this rather than recomputing per build,
/// otherwise every `cargo build` would flip the stale flag.
pub use crate::bibtex::KEY_ALGO_HASH as ALGO_HASH;

/// Schema-version discriminant. Bump only on incompatible field
/// changes — additive optional fields don't require a bump.
pub const FORMAT_VERSION: &str = "1";

/// `format` discriminant for BibTeX-flavored snapshots. The CSL-JSON
/// variant in #135 will introduce `"csl-json"` alongside; keeping this
/// a string keeps the schema open without a serde enum migration.
pub const FORMAT_BIBTEX: &str = "bibtex";

/// Sidecar JSON written next to the exported `.bib` (path =
/// `<output>.scitadel-bib.lock`). One sidecar per output file —
/// per-question scope means several may live in one repo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BibLockfile {
    /// Research question this snapshot was scoped to.
    pub question_id: String,
    /// Reader identity at snapshot time. Mirrors annotation/star
    /// scoping — different readers may produce different shortlists.
    pub reader: String,
    /// SHA-256 of the lex-sorted `paper_id\n` list. The drift anchor.
    pub shortlist_hash: String,
    /// SHA-256 of the emitted `.bib` bytes. Cheap re-emit skip.
    pub content_hash: String,
    /// `scitadel` binary version that produced this snapshot. Drives
    /// the stale exit code when the binary moves forward.
    pub scitadel_version: String,
    /// Pinned key-generation algorithm hash from ADR-006. Drives the
    /// stale exit code when the algorithm itself drifts.
    pub algo_hash: String,
    /// Output flavor — `"bibtex"` here. `#135` adds `"csl-json"`.
    pub format: String,
    /// Schema version of this lockfile.
    pub format_version: String,
    /// RFC3339 timestamp of when the snapshot was written. Excluded
    /// from byte-determinism guarantees — snapshot-twice-same-bytes
    /// holds for everything else.
    pub generated_at: String,
}

impl BibLockfile {
    /// Construct a fresh lockfile for a BibTeX snapshot. `paper_ids`
    /// must be the shortlist's paper IDs; ordering is normalized
    /// internally so callers don't have to think about it.
    #[must_use]
    pub fn new_bibtex(
        question_id: impl Into<String>,
        reader: impl Into<String>,
        paper_ids: &[String],
        content: &str,
    ) -> Self {
        Self {
            question_id: question_id.into(),
            reader: reader.into(),
            shortlist_hash: shortlist_hash(paper_ids),
            content_hash: content_hash(content),
            scitadel_version: env!("CARGO_PKG_VERSION").to_string(),
            algo_hash: ALGO_HASH.to_string(),
            format: FORMAT_BIBTEX.to_string(),
            format_version: FORMAT_VERSION.to_string(),
            generated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Pretty-printed JSON serialization with a trailing newline so
    /// editors don't dirty the file on first open.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }

    /// Parse from JSON. Errors propagate the serde failure verbatim.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// SHA-256 of the lex-sorted `paper_id\n` list. Sort happens here so
/// callers cannot accidentally pass insertion-ordered IDs and produce
/// a non-canonical hash. Returns `sha256:<hex>` so the field stays
/// self-describing in the JSON.
#[must_use]
pub fn shortlist_hash(paper_ids: &[String]) -> String {
    let sorted: BTreeSet<&String> = paper_ids.iter().collect();
    let mut hasher = Sha256::new();
    for id in sorted {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    format!("sha256:{}", hex_lower(&hasher.finalize()))
}

/// SHA-256 of the emitted bytes. Returned as `sha256:<hex>`.
#[must_use]
pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("sha256:{}", hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortlist_hash_is_order_independent() {
        let a = shortlist_hash(&["p-zeta".into(), "p-alpha".into(), "p-mu".into()]);
        let b = shortlist_hash(&["p-mu".into(), "p-alpha".into(), "p-zeta".into()]);
        assert_eq!(a, b, "input order must not affect shortlist_hash");
    }

    #[test]
    fn shortlist_hash_changes_on_membership_change() {
        let a = shortlist_hash(&["p-1".into(), "p-2".into()]);
        let b = shortlist_hash(&["p-1".into(), "p-2".into(), "p-3".into()]);
        assert_ne!(a, b);
    }

    #[test]
    fn shortlist_hash_format_self_describing() {
        let h = shortlist_hash(&["p-1".into()]);
        assert!(h.starts_with("sha256:"), "got: {h}");
        // 7 prefix + 64 hex chars
        assert_eq!(h.len(), 7 + 64);
    }

    #[test]
    fn content_hash_format_self_describing() {
        let h = content_hash("@article{x,\n  title={Y},\n}\n");
        assert!(h.starts_with("sha256:"));
        assert_eq!(h.len(), 7 + 64);
    }

    #[test]
    fn content_hash_distinguishes_inputs() {
        let a = content_hash("foo");
        let b = content_hash("bar");
        assert_ne!(a, b);
    }

    #[test]
    fn lockfile_round_trips_via_serde() {
        let lock = BibLockfile::new_bibtex(
            "q-abc",
            "lars",
            &["p-1".into(), "p-2".into()],
            "@article{x,\n}\n",
        );
        let json = lock.to_json().unwrap();
        let back = BibLockfile::from_json(&json).unwrap();
        assert_eq!(lock, back);
    }

    #[test]
    fn lockfile_has_all_required_fields() {
        let lock = BibLockfile::new_bibtex("q", "r", &[], "");
        let json = lock.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        for key in [
            "question_id",
            "reader",
            "shortlist_hash",
            "content_hash",
            "scitadel_version",
            "algo_hash",
            "format",
            "format_version",
            "generated_at",
        ] {
            assert!(v.get(key).is_some(), "missing field: {key}");
        }
        assert_eq!(v["format"], "bibtex");
        assert_eq!(v["format_version"], "1");
    }

    /// Sourcing `algo_hash` from the same `KEY_ALGO_HASH` constant the
    /// exporter pins guarantees `bib verify` and the algorithm itself
    /// can never disagree about freshness — they read the same byte.
    #[test]
    fn algo_hash_is_sourced_from_key_algo_hash_constant() {
        assert_eq!(ALGO_HASH, crate::bibtex::KEY_ALGO_HASH);
        let lock = BibLockfile::new_bibtex("q", "r", &[], "");
        assert_eq!(lock.algo_hash, crate::bibtex::KEY_ALGO_HASH);
    }
}
