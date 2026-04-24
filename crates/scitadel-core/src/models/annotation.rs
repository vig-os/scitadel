//! Annotations (highlights + threaded notes) anchored to paper text.
//!
//! Follows the W3C Web Annotation selector pattern: a single annotation
//! may carry multiple selectors (position, quote + context, sentence id),
//! and the resolver tries them in order on open. Threading is self-
//! referential via `parent_id`; replies inherit the root's anchor.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{AnnotationId, PaperId, QuestionId};

/// Status of the anchor after last resolve attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorStatus {
    #[default]
    /// Character-range match; the quote still lives at the same offset.
    Ok,
    /// The exact offsets moved but the quote (or sentence id) still matches.
    Drifted,
    /// None of the selectors matched — needs user re-anchoring.
    Orphan,
}

impl AnchorStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Drifted => "drifted",
            Self::Orphan => "orphan",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "drifted" => Some(Self::Drifted),
            "orphan" => Some(Self::Orphan),
            _ => None,
        }
    }
}

/// Multi-selector anchor. Any field may be `None`; the resolver falls
/// through: position → quote + context → sentence id → orphan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// TextPositionSelector: fast, fragile. `(start, end)` in chars.
    pub char_range: Option<(usize, usize)>,
    /// TextQuoteSelector body.
    pub quote: Option<String>,
    /// Context before the quote — used for disambiguation.
    pub prefix: Option<String>,
    /// Context after the quote.
    pub suffix: Option<String>,
    /// SHA1 of the normalized sentence containing the quote.
    pub sentence_id: Option<String>,
    /// Which paper-text extraction version this was anchored against.
    pub source_version: Option<String>,
    /// Last-known resolution status; updated on open.
    pub status: AnchorStatus,
}

impl Anchor {
    /// Is this an orphan that requires user re-anchoring?
    #[must_use]
    pub fn is_orphan(&self) -> bool {
        matches!(self.status, AnchorStatus::Orphan)
    }
}

/// One annotation. May be a root (with an anchor) or a reply (parent_id set,
/// anchor empty; the root's anchor is the canonical one for rendering).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: AnnotationId,
    /// `None` = root (carries the anchor). `Some` = reply to that ID.
    pub parent_id: Option<AnnotationId>,
    pub paper_id: PaperId,
    pub question_id: Option<QuestionId>,
    pub anchor: Anchor,
    pub note: String,
    pub color: Option<String>,
    pub tags: Vec<String>,
    /// Identity string — `$USER` for TUI writes, required for MCP writes.
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Soft-delete tombstone. None = live.
    pub deleted_at: Option<DateTime<Utc>>,
}

impl Annotation {
    /// Build a new root-level annotation with the given anchor.
    #[must_use]
    pub fn new_root(paper_id: PaperId, author: String, note: String, anchor: Anchor) -> Self {
        let now = Utc::now();
        Self {
            id: AnnotationId::new(),
            parent_id: None,
            paper_id,
            question_id: None,
            anchor,
            note,
            color: None,
            tags: Vec::new(),
            author,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    /// Build a new reply whose anchor is empty (inherits from root).
    #[must_use]
    pub fn new_reply(parent: &Annotation, author: String, note: String) -> Self {
        let now = Utc::now();
        Self {
            id: AnnotationId::new(),
            parent_id: Some(parent.id.clone()),
            paper_id: parent.paper_id.clone(),
            question_id: parent.question_id.clone(),
            anchor: Anchor::default(),
            note,
            color: None,
            tags: Vec::new(),
            author,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    /// True if this is a reply to another annotation.
    #[must_use]
    pub fn is_reply(&self) -> bool {
        self.parent_id.is_some()
    }

    /// True if this annotation has been soft-deleted.
    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// Per-reader read receipt; composite key (annotation_id, reader).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationRead {
    pub annotation_id: AnnotationId,
    pub reader: String,
    pub seen_at: DateTime<Utc>,
}

/// Normalize a sentence for sentence-id hashing.
///
/// Per ADR-004: NFKC compose (folds ligatures: ﬁ → fi, ﬂ → fl), Unicode
/// lowercase, then collapse all Unicode whitespace runs to a single
/// ASCII space and trim. Two sentences that differ only in case,
/// whitespace, or ligature presentation hash to the same value.
#[must_use]
pub fn normalize_sentence(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    // NFKC: compatibility decomposition + canonical composition.
    let composed: String = s.nfkc().collect();
    let lowered: String = composed.chars().flat_map(char::to_lowercase).collect();
    let mut out = String::with_capacity(lowered.len());
    let mut prev_was_space = true; // collapses leading whitespace
    for ch in lowered.chars() {
        if ch.is_whitespace() {
            if !prev_was_space {
                out.push(' ');
                prev_was_space = true;
            }
        } else {
            out.push(ch);
            prev_was_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// SHA1 hex of the normalized sentence — stable identifier the
/// resolver can compare against sentences extracted from current
/// paper text. See `normalize_sentence` and ADR-004 for the
/// normalization spec.
#[must_use]
pub fn sentence_id(s: &str) -> String {
    use sha1::{Digest, Sha1};
    let normalized = normalize_sentence(s);
    let mut hasher = Sha1::new();
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(40);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_inherits_paper_and_question() {
        let paper_id: PaperId = "p1".into();
        let root = Annotation::new_root(
            paper_id.clone(),
            "lars".into(),
            "interesting passage".into(),
            Anchor {
                quote: Some("neutron energy".into()),
                ..Anchor::default()
            },
        );
        let reply = Annotation::new_reply(&root, "claude".into(), "agreed; see 4.2".into());
        assert_eq!(reply.paper_id, paper_id);
        assert_eq!(reply.parent_id.as_ref(), Some(&root.id));
        assert!(reply.anchor.quote.is_none(), "replies inherit anchor");
    }

    #[test]
    fn anchor_status_round_trip() {
        for s in [
            AnchorStatus::Ok,
            AnchorStatus::Drifted,
            AnchorStatus::Orphan,
        ] {
            assert_eq!(AnchorStatus::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn orphan_flag() {
        let mut a = Anchor::default();
        assert!(!a.is_orphan());
        a.status = AnchorStatus::Orphan;
        assert!(a.is_orphan());
    }

    #[test]
    fn normalize_collapses_whitespace_and_lowercases() {
        assert_eq!(normalize_sentence("  Hello   WORLD\n"), "hello world");
    }

    #[test]
    fn normalize_folds_ligatures_via_nfkc() {
        // U+FB01 (ﬁ) → "fi" under NFKC, so "ef + ﬁ + cient" → "efficient".
        assert_eq!(normalize_sentence("ef\u{FB01}cient"), "efficient");
    }

    #[test]
    fn sentence_id_is_stable_under_whitespace_and_case() {
        let a = sentence_id("Hello   World");
        let b = sentence_id("hello world");
        let c = sentence_id("HELLO\tWORLD");
        assert_eq!(a, b);
        assert_eq!(b, c);
        // Length of SHA1 hex.
        assert_eq!(a.len(), 40);
    }

    #[test]
    fn sentence_id_changes_when_content_does() {
        assert_ne!(sentence_id("hello world"), sentence_id("hello mars"));
    }
}
