//! SQLite-backed repository for annotations (#49 iter 2, #96 resolver).
//!
//! Covers CRUD, threaded reply loading, and the four-step W3C-style
//! anchor resolver:
//!
//! 1. position (`char_range` + bounds-check)
//! 2. quote with prefix/suffix context disambiguation
//! 3. fuzzy quote match (Jaro-Winkler over a sliding window)
//! 4. sentence-id (SHA1 of normalized sentence; see ADR-004)
//!
//! Failure of all four selectors yields `AnchorStatus::Orphan`.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use scitadel_core::models::{Anchor, AnchorStatus, Annotation, AnnotationId, PaperId, QuestionId};

use crate::error::DbError;
use crate::sqlite::Database;

#[derive(Clone)]
pub struct SqliteAnnotationRepository {
    db: Database,
}

impl SqliteAnnotationRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Insert a new annotation. Caller is responsible for building the
    /// `Annotation` (see `Annotation::new_root` / `new_reply`).
    pub fn create(&self, annotation: &Annotation) -> Result<(), DbError> {
        let conn = self.db.conn()?;
        conn.execute(
            "INSERT INTO annotations
                (id, parent_id, paper_id, question_id,
                 char_start, char_end, quote, prefix, suffix,
                 sentence_id, source_version, anchor_status,
                 note, color, tags_json, author,
                 created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4,
                     ?5, ?6, ?7, ?8, ?9,
                     ?10, ?11, ?12,
                     ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19)",
            params![
                annotation.id.as_str(),
                annotation.parent_id.as_ref().map(AnnotationId::as_str),
                annotation.paper_id.as_str(),
                annotation.question_id.as_ref().map(QuestionId::as_str),
                annotation.anchor.char_range.map(|(s, _)| s as i64),
                annotation.anchor.char_range.map(|(_, e)| e as i64),
                annotation.anchor.quote,
                annotation.anchor.prefix,
                annotation.anchor.suffix,
                annotation.anchor.sentence_id,
                annotation.anchor.source_version,
                annotation.anchor.status.as_str(),
                annotation.note,
                annotation.color,
                serde_json::to_string(&annotation.tags).unwrap_or_else(|_| "[]".into()),
                annotation.author,
                annotation.created_at.to_rfc3339(),
                annotation.updated_at.to_rfc3339(),
                annotation.deleted_at.map(|d| d.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    /// Fetch an annotation by ID (live rows only).
    pub fn get(&self, id: &str) -> Result<Option<Annotation>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt =
            conn.prepare("SELECT * FROM annotations WHERE id = ?1 AND deleted_at IS NULL")?;
        let out = stmt.query_row(params![id], row_to_annotation).optional()?;
        Ok(out)
    }

    /// All live annotations anchored to a paper (roots + replies).
    pub fn list_by_paper(&self, paper_id: &str) -> Result<Vec<Annotation>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM annotations
             WHERE paper_id = ?1 AND deleted_at IS NULL
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![paper_id], row_to_annotation)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// All live replies to a specific root annotation, ordered oldest-first.
    pub fn list_replies(&self, parent_id: &str) -> Result<Vec<Annotation>, DbError> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM annotations
             WHERE parent_id = ?1 AND deleted_at IS NULL
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![parent_id], row_to_annotation)?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Update mutable fields (note / color / tags). Anchor is updated
    /// separately via `update_anchor` since it has its own lifecycle.
    pub fn update_note(
        &self,
        id: &str,
        note: &str,
        color: Option<&str>,
        tags: &[String],
    ) -> Result<(), DbError> {
        let conn = self.db.conn()?;
        conn.execute(
            "UPDATE annotations
             SET note = ?1, color = ?2, tags_json = ?3, updated_at = ?4
             WHERE id = ?5",
            params![
                note,
                color,
                serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()),
                Utc::now().to_rfc3339(),
                id,
            ],
        )?;
        Ok(())
    }

    /// Persist the resolver's updated anchor state. Called after
    /// `resolve_anchor` runs on paper-open.
    pub fn update_anchor(&self, id: &str, anchor: &Anchor) -> Result<(), DbError> {
        let conn = self.db.conn()?;
        conn.execute(
            "UPDATE annotations
             SET char_start = ?1, char_end = ?2,
                 anchor_status = ?3, updated_at = ?4
             WHERE id = ?5",
            params![
                anchor.char_range.map(|(s, _)| s as i64),
                anchor.char_range.map(|(_, e)| e as i64),
                anchor.status.as_str(),
                Utc::now().to_rfc3339(),
                id,
            ],
        )?;
        Ok(())
    }

    /// Soft-delete — tombstones the row so replies still point at
    /// something, and `list_*` queries skip it.
    pub fn soft_delete(&self, id: &str) -> Result<(), DbError> {
        let conn = self.db.conn()?;
        conn.execute(
            "UPDATE annotations SET deleted_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Record that `reader` has seen the current state of each annotation.
    /// Upserts so repeat calls bump `seen_at`.
    pub fn mark_seen(&self, annotation_ids: &[&str], reader: &str) -> Result<(), DbError> {
        if annotation_ids.is_empty() {
            return Ok(());
        }
        let mut conn = self.db.conn()?;
        let tx = conn.transaction()?;
        let now = Utc::now().to_rfc3339();
        for id in annotation_ids {
            tx.execute(
                "INSERT INTO annotation_reads (annotation_id, reader, seen_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(annotation_id, reader) DO UPDATE SET seen_at = excluded.seen_at",
                params![id, reader, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Mark the thread rooted at `root_id` (root + all live replies) as
    /// seen by `reader`.
    pub fn mark_thread_seen(&self, root_id: &str, reader: &str) -> Result<(), DbError> {
        let replies = self.list_replies(root_id)?;
        let mut ids: Vec<&str> = replies.iter().map(|a| a.id.as_str()).collect();
        ids.push(root_id);
        self.mark_seen(&ids, reader)
    }

    /// Annotations the `reader` hasn't seen since the last modification.
    /// Optional `paper_id` scopes the query. Uses a LEFT JOIN so rows
    /// with no receipt count as unread; rows whose `seen_at` is older
    /// than `updated_at` also count (the annotation changed since last
    /// view).
    pub fn list_unread(
        &self,
        reader: &str,
        paper_id: Option<&str>,
    ) -> Result<Vec<Annotation>, DbError> {
        let conn = self.db.conn()?;
        let (sql, rows) = if let Some(pid) = paper_id {
            let mut stmt = conn.prepare(
                "SELECT a.* FROM annotations a
                 LEFT JOIN annotation_reads r
                   ON r.annotation_id = a.id AND r.reader = ?1
                 WHERE a.paper_id = ?2
                   AND a.deleted_at IS NULL
                   AND (r.seen_at IS NULL OR r.seen_at < a.updated_at)
                 ORDER BY a.created_at ASC",
            )?;
            let rows = stmt
                .query_map(params![reader, pid], row_to_annotation)?
                .filter_map(Result::ok)
                .collect::<Vec<_>>();
            ("scoped", rows)
        } else {
            let mut stmt = conn.prepare(
                "SELECT a.* FROM annotations a
                 LEFT JOIN annotation_reads r
                   ON r.annotation_id = a.id AND r.reader = ?1
                 WHERE a.deleted_at IS NULL
                   AND (r.seen_at IS NULL OR r.seen_at < a.updated_at)
                 ORDER BY a.created_at ASC",
            )?;
            let rows = stmt
                .query_map(params![reader], row_to_annotation)?
                .filter_map(Result::ok)
                .collect::<Vec<_>>();
            ("all", rows)
        };
        let _ = sql; // kept for potential future logging
        Ok(rows)
    }
}

fn row_to_annotation(row: &rusqlite::Row) -> rusqlite::Result<Annotation> {
    let char_start: Option<i64> = row.get("char_start")?;
    let char_end: Option<i64> = row.get("char_end")?;
    let char_range = match (char_start, char_end) {
        (Some(s), Some(e)) => Some((s as usize, e as usize)),
        _ => None,
    };
    let anchor_status_str: Option<String> = row.get("anchor_status")?;
    let anchor = Anchor {
        char_range,
        quote: row.get("quote")?,
        prefix: row.get("prefix")?,
        suffix: row.get("suffix")?,
        sentence_id: row.get("sentence_id")?,
        source_version: row.get("source_version")?,
        status: anchor_status_str
            .as_deref()
            .and_then(AnchorStatus::parse)
            .unwrap_or_default(),
    };

    let tags_json: String = row.get("tags_json")?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

    let parent_id: Option<String> = row.get("parent_id")?;
    let question_id: Option<String> = row.get("question_id")?;
    let created_at: String = row.get("created_at")?;
    let updated_at: String = row.get("updated_at")?;
    let deleted_at: Option<String> = row.get("deleted_at")?;

    Ok(Annotation {
        id: AnnotationId::from(row.get::<_, String>("id")?),
        parent_id: parent_id.map(AnnotationId::from),
        paper_id: PaperId::from(row.get::<_, String>("paper_id")?),
        question_id: question_id.map(QuestionId::from),
        anchor,
        note: row.get("note")?,
        color: row.get("color")?,
        tags,
        author: row.get("author")?,
        created_at: parse_dt(&created_at),
        updated_at: parse_dt(&updated_at),
        deleted_at: deleted_at.as_deref().map(parse_dt),
    })
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

/// Default fuzzy-match threshold (Jaro-Winkler similarity in [0,1]).
/// Anchors at or above this score are accepted as `Drifted`. See
/// `resolve_anchor_with_threshold` for tuning.
pub const FUZZY_THRESHOLD: f64 = 0.9;

/// Resolve an anchor against current paper text, updating `status` and
/// (if the quote shifted) `char_range` in place. Four-step W3C-style
/// pipeline (#96):
///
/// 1. **Position**: `char_range` still hits the same `quote` → `Ok`.
///    Bounds-checked; out-of-range offsets fall through, never panic.
/// 2. **Quote + prefix/suffix context**: every occurrence of `quote`
///    in `text` is scored by how well its surroundings match the
///    stored `prefix` / `suffix`; the best-scoring occurrence wins.
///    With a single occurrence and no context, behaves like a plain
///    substring search → `Drifted`.
/// 3. **Fuzzy quote match**: sliding window the size of `quote` over
///    `text`; Jaro-Winkler ≥ `FUZZY_THRESHOLD` → `Drifted`. Catches
///    one-word publisher edits that would otherwise orphan.
/// 4. **Sentence-id**: split text into sentences, hash each via
///    `sentence_id()`, and re-anchor on a match. Survives quote
///    rewrites that preserve the surrounding sentence.
///
/// Returns `Orphan` only when all four selectors fail.
pub fn resolve_anchor(anchor: &mut Anchor, text: &str) -> AnchorStatus {
    resolve_anchor_with_threshold(anchor, text, FUZZY_THRESHOLD)
}

pub fn resolve_anchor_with_threshold(
    anchor: &mut Anchor,
    text: &str,
    fuzzy_threshold: f64,
) -> AnchorStatus {
    // Step 1: position selector — bounds-checked.
    if let (Some((start, end)), Some(quote)) = (anchor.char_range, anchor.quote.as_ref())
        && let Some(slice) = char_slice(text, start, end)
        && &slice == quote
    {
        anchor.status = AnchorStatus::Ok;
        return AnchorStatus::Ok;
    }

    // Step 2: quote with prefix/suffix disambiguation.
    if let Some(quote) = anchor.quote.as_ref()
        && let Some((sc, ec)) = find_with_context(
            text,
            quote,
            anchor.prefix.as_deref(),
            anchor.suffix.as_deref(),
        )
    {
        anchor.char_range = Some((sc, ec));
        anchor.status = AnchorStatus::Drifted;
        return AnchorStatus::Drifted;
    }

    // Step 3: fuzzy quote match (sliding window).
    if let Some(quote) = anchor.quote.as_ref()
        && let Some((sc, ec)) = fuzzy_find(text, quote, fuzzy_threshold)
    {
        anchor.char_range = Some((sc, ec));
        anchor.status = AnchorStatus::Drifted;
        return AnchorStatus::Drifted;
    }

    // Step 4: sentence-id fallback.
    if let Some(sid) = anchor.sentence_id.as_ref()
        && let Some((sc, ec)) = find_sentence_by_id(text, sid)
    {
        anchor.char_range = Some((sc, ec));
        anchor.status = AnchorStatus::Drifted;
        return AnchorStatus::Drifted;
    }

    anchor.status = AnchorStatus::Orphan;
    AnchorStatus::Orphan
}

/// Slice `text` by char positions, returning `None` if the requested
/// range is malformed (start > end) or beyond the text. Avoids the
/// panic the old resolver hit on out-of-bounds rows (#96 gap 4).
fn char_slice(text: &str, start: usize, end: usize) -> Option<String> {
    if end < start {
        return None;
    }
    let want = end - start;
    let collected: String = text.chars().skip(start).take(want).collect();
    if collected.chars().count() == want {
        Some(collected)
    } else {
        None
    }
}

/// Find every (start_char, end_char) where `quote` occurs in `text`.
/// Char-position aware — matches step over multibyte boundaries cleanly.
fn find_all(text: &str, quote: &str) -> Vec<(usize, usize)> {
    if quote.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let qlen_chars = quote.chars().count();
    let mut search_byte = 0;
    while let Some(rel) = text[search_byte..].find(quote) {
        let abs = search_byte + rel;
        let start_char = text[..abs].chars().count();
        out.push((start_char, start_char + qlen_chars));
        search_byte = abs + quote.len(); // non-overlapping; quote is non-empty
    }
    out
}

/// Pick the occurrence whose surrounding context best matches the
/// stored `prefix` / `suffix`. With a single hit and no context, it's
/// a plain substring lookup; with multiple hits, the prefix-suffix
/// score breaks the tie.
fn find_with_context(
    text: &str,
    quote: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
) -> Option<(usize, usize)> {
    let occurrences = find_all(text, quote);
    if occurrences.is_empty() {
        return None;
    }
    if occurrences.len() == 1 || (prefix.is_none() && suffix.is_none()) {
        return Some(occurrences[0]);
    }

    let chars: Vec<char> = text.chars().collect();
    occurrences
        .into_iter()
        .max_by_key(|&(sc, ec)| context_score(&chars, sc, ec, prefix, suffix))
}

/// Score a candidate's surroundings against the stored prefix/suffix.
/// Counts characters that match starting from the inside out (the
/// chars adjacent to the match are most load-bearing).
fn context_score(
    chars: &[char],
    start: usize,
    end: usize,
    prefix: Option<&str>,
    suffix: Option<&str>,
) -> i64 {
    let mut score = 0i64;
    if let Some(p) = prefix {
        let want: Vec<char> = p.chars().collect();
        let max = want.len().min(start);
        for i in 0..max {
            // chars[start - 1 - i] vs want[want.len() - 1 - i]
            if chars[start - 1 - i] == want[want.len() - 1 - i] {
                score += 1;
            } else {
                break;
            }
        }
    }
    if let Some(s) = suffix {
        let want: Vec<char> = s.chars().collect();
        let max = want.len().min(chars.len().saturating_sub(end));
        for i in 0..max {
            if chars[end + i] == want[i] {
                score += 1;
            } else {
                break;
            }
        }
    }
    score
}

/// Sliding-window fuzzy match. Walks character-aligned windows the
/// size of `quote` and returns the highest-scoring window that meets
/// `threshold` (Jaro-Winkler in [0,1]).
fn fuzzy_find(text: &str, quote: &str, threshold: f64) -> Option<(usize, usize)> {
    if quote.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let qlen = quote.chars().count();
    if chars.len() < qlen {
        return None;
    }

    let mut best: Option<(usize, f64)> = None;
    for start in 0..=chars.len() - qlen {
        let window: String = chars[start..start + qlen].iter().collect();
        let score = strsim::jaro_winkler(&window, quote);
        if score >= threshold && best.is_none_or(|(_, b)| score > b) {
            best = Some((start, score));
        }
    }
    best.map(|(start, _)| (start, start + qlen))
}

/// Find the sentence in `text` whose `sentence_id` matches `sid`.
/// Sentence boundaries are simple terminator-based (`. ! ?`) — good
/// enough for paper bodies and abstracts; ADR-004 calls out that
/// proper ICU sentence segmentation is a follow-up.
fn find_sentence_by_id(text: &str, sid: &str) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentence_start_char = 0;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let is_terminator = matches!(ch, '.' | '!' | '?');
        let is_end = i + 1 == chars.len();
        if is_terminator || is_end {
            let end = if is_end { chars.len() } else { i + 1 };
            let sentence: String = chars[sentence_start_char..end].iter().collect();
            let trimmed = sentence.trim();
            if !trimmed.is_empty() && scitadel_core::models::sentence_id(trimmed) == sid {
                // Map back to the trimmed sentence's char range inside `text`.
                let leading_ws = sentence.chars().take_while(|c| c.is_whitespace()).count();
                let trailing_ws = sentence
                    .chars()
                    .rev()
                    .take_while(|c| c.is_whitespace())
                    .count();
                let trimmed_start = sentence_start_char + leading_ws;
                let trimmed_end = end - trailing_ws;
                if trimmed_end > trimmed_start {
                    return Some((trimmed_start, trimmed_end));
                }
            }
            // Advance past the terminator into the next sentence.
            sentence_start_char = end;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::Annotation;

    fn fresh_db_with_paper() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO papers (id, title, created_at, updated_at)
             VALUES ('p1', 't', datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();
        db
    }

    fn sample_root() -> Annotation {
        Annotation::new_root(
            PaperId::from("p1"),
            "lars".into(),
            "important passage".into(),
            Anchor {
                char_range: Some((10, 25)),
                quote: Some("neutron energy".into()),
                ..Anchor::default()
            },
        )
    }

    /// Offline-safe invariant (#51). Every annotation write path
    /// (`create`, replies, `update_note`, `soft_delete`) must be purely
    /// local — no network, no auth probe, no reqwest. The 2-pane
    /// workflow makes this trust-critical: a user on a plane still
    /// captures their reading notes; the TUI's offline badge only
    /// gates network-requiring operations (search / download), not
    /// annotations.
    ///
    /// This test locks that invariant in: the entire annotation
    /// lifecycle round-trips through a fresh in-memory SQLite DB with
    /// no `reqwest::Client`, no environment, no adapters instantiated.
    /// If a future refactor introduces a network dep on this path,
    /// the construction of that dep will either force this test to
    /// change or will be catchable by review.
    #[test]
    fn annotation_writes_are_offline_safe() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);

        // Create root → reply → update root note → soft-delete reply.
        // If any of these silently required network access, the call
        // chain wouldn't compile (no reqwest in this crate's deps).
        let root = sample_root();
        repo.create(&root).unwrap();
        let reply = Annotation::new_reply(&root, "claude".into(), "seconded".into());
        repo.create(&reply).unwrap();
        repo.update_note(root.id.as_str(), "edited offline", None, &[])
            .unwrap();
        repo.soft_delete(reply.id.as_str()).unwrap();

        // Survivors visible on next read.
        let all = repo.list_by_paper("p1").unwrap();
        assert_eq!(all.len(), 1, "root survives; reply tombstoned out");
        assert_eq!(all[0].note, "edited offline");
    }

    #[test]
    fn create_and_get_roundtrip() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let root = sample_root();
        repo.create(&root).unwrap();

        let loaded = repo.get(root.id.as_str()).unwrap().expect("present");
        assert_eq!(loaded.note, "important passage");
        assert_eq!(loaded.anchor.char_range, Some((10, 25)));
        assert_eq!(loaded.anchor.quote.as_deref(), Some("neutron energy"));
    }

    #[test]
    fn replies_threaded_under_root() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let root = sample_root();
        repo.create(&root).unwrap();
        let reply = Annotation::new_reply(&root, "claude".into(), "see fig 4".into());
        repo.create(&reply).unwrap();

        let replies = repo.list_replies(root.id.as_str()).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].note, "see fig 4");
    }

    #[test]
    fn soft_delete_hides_from_listings_but_thread_preserved() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let root = sample_root();
        repo.create(&root).unwrap();
        let reply = Annotation::new_reply(&root, "claude".into(), "yep".into());
        repo.create(&reply).unwrap();

        repo.soft_delete(root.id.as_str()).unwrap();

        // Root is hidden from get() and list_by_paper()
        assert!(repo.get(root.id.as_str()).unwrap().is_none());
        assert!(
            repo.list_by_paper("p1")
                .unwrap()
                .iter()
                .all(|a| a.id != root.id)
        );
        // Reply still points at the (soft-deleted) root, so the thread is
        // recoverable if we ever want to undelete.
        let replies = repo.list_replies(root.id.as_str()).unwrap();
        assert_eq!(replies.len(), 1);
    }

    #[test]
    fn update_note_persists() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let root = sample_root();
        repo.create(&root).unwrap();

        repo.update_note(
            root.id.as_str(),
            "new note",
            Some("blue"),
            &["tag1".into(), "tag2".into()],
        )
        .unwrap();

        let loaded = repo.get(root.id.as_str()).unwrap().unwrap();
        assert_eq!(loaded.note, "new note");
        assert_eq!(loaded.color.as_deref(), Some("blue"));
        assert_eq!(loaded.tags, vec!["tag1".to_string(), "tag2".to_string()]);
    }

    // ---- Resolver tests ----

    #[test]
    fn resolver_ok_when_text_unchanged() {
        // "abcde" at offsets (1,4) is "bcd".
        let mut a = Anchor {
            char_range: Some((1, 4)),
            quote: Some("bcd".into()),
            ..Anchor::default()
        };
        assert_eq!(resolve_anchor(&mut a, "abcde"), AnchorStatus::Ok);
    }

    #[test]
    fn resolver_drifted_when_quote_moved() {
        // Same quote, shifted 2 chars to the right.
        let mut a = Anchor {
            char_range: Some((1, 4)),
            quote: Some("bcd".into()),
            ..Anchor::default()
        };
        assert_eq!(resolve_anchor(&mut a, "xxabcde"), AnchorStatus::Drifted);
        assert_eq!(a.char_range, Some((3, 6)));
        assert_eq!(a.status, AnchorStatus::Drifted);
    }

    #[test]
    fn resolver_orphan_when_quote_missing() {
        let mut a = Anchor {
            char_range: Some((1, 4)),
            quote: Some("bcd".into()),
            ..Anchor::default()
        };
        assert_eq!(
            resolve_anchor(&mut a, "nothing to see"),
            AnchorStatus::Orphan
        );
    }

    // ---- Read-receipt tests ----

    #[test]
    fn unread_includes_rows_never_seen() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let a = sample_root();
        repo.create(&a).unwrap();
        let unread = repo.list_unread("lars", Some("p1")).unwrap();
        assert_eq!(unread.len(), 1);
    }

    #[test]
    fn unread_excludes_rows_seen_after_update() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let a = sample_root();
        repo.create(&a).unwrap();
        repo.mark_seen(&[a.id.as_str()], "lars").unwrap();
        let unread = repo.list_unread("lars", Some("p1")).unwrap();
        assert!(unread.is_empty(), "should be no unread after mark_seen");
    }

    #[test]
    fn unread_reappears_after_annotation_is_updated() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let a = sample_root();
        repo.create(&a).unwrap();
        repo.mark_seen(&[a.id.as_str()], "lars").unwrap();
        // Pause past the 1-second rfc3339 resolution the repo uses.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        repo.update_note(a.id.as_str(), "edited note", None, &[])
            .unwrap();
        let unread = repo.list_unread("lars", Some("p1")).unwrap();
        assert_eq!(unread.len(), 1, "edit should resurface the row as unread");
    }

    #[test]
    fn mark_thread_seen_covers_root_and_replies() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let root = sample_root();
        repo.create(&root).unwrap();
        let reply = Annotation::new_reply(&root, "claude".into(), "follow-up".into());
        repo.create(&reply).unwrap();

        repo.mark_thread_seen(root.id.as_str(), "lars").unwrap();
        let unread = repo.list_unread("lars", Some("p1")).unwrap();
        assert!(unread.is_empty());
    }

    #[test]
    fn independent_readers_track_state_independently() {
        let db = fresh_db_with_paper();
        let repo = SqliteAnnotationRepository::new(db);
        let a = sample_root();
        repo.create(&a).unwrap();
        repo.mark_seen(&[a.id.as_str()], "lars").unwrap();
        assert!(repo.list_unread("lars", Some("p1")).unwrap().is_empty());
        assert_eq!(repo.list_unread("claude", Some("p1")).unwrap().len(), 1);
    }

    #[test]
    fn resolver_handles_multibyte_chars() {
        // U+2019 (curly apostrophe) is 3 bytes / 1 char.
        let text = "D\u{2019}Ippolito wrote that...";
        let quote = "D\u{2019}Ippolito";
        let mut a = Anchor {
            char_range: Some((0, quote.chars().count())),
            quote: Some(quote.into()),
            ..Anchor::default()
        };
        assert_eq!(resolve_anchor(&mut a, text), AnchorStatus::Ok);
    }

    // ---- #96 multi-selector resolver tests ----

    #[test]
    fn resolver_uses_prefix_to_disambiguate_collision() {
        // "the model" appears twice; suffix " was trained" picks the second.
        let text = "Initially the model failed. Then the model was trained on more data.";
        let mut a = Anchor {
            char_range: None,
            quote: Some("the model".into()),
            prefix: None,
            suffix: Some(" was trained".into()),
            ..Anchor::default()
        };
        assert_eq!(resolve_anchor(&mut a, text), AnchorStatus::Drifted);
        let (s, e) = a.char_range.unwrap();
        assert_eq!(&text[s..e], "the model");
        // Specifically the *second* occurrence.
        assert!(s > 20, "expected the second occurrence at s>20, got s={s}");
    }

    #[test]
    fn resolver_falls_back_to_fuzzy_on_minor_edit() {
        // Quote was "the network was deep"; publisher edited to "the
        // network was very deep" — substring fails, fuzzy still hits.
        let text = "We argued the network was very deep enough to overfit.";
        let mut a = Anchor {
            char_range: None,
            quote: Some("the network was deep".into()),
            ..Anchor::default()
        };
        // Use a permissive threshold so the test isn't sensitive to
        // strsim version drift.
        let s = resolve_anchor_with_threshold(&mut a, text, 0.85);
        assert_eq!(
            s,
            AnchorStatus::Drifted,
            "fuzzy match should drift, got {s:?}"
        );
    }

    #[test]
    fn resolver_returns_orphan_when_offsets_oob_and_quote_absent() {
        // char_range out of bounds, quote not present in text — must
        // return Orphan instead of panicking. (#96 gap 4)
        let mut a = Anchor {
            char_range: Some((9000, 9100)),
            quote: Some("vanished".into()),
            ..Anchor::default()
        };
        assert_eq!(
            resolve_anchor(&mut a, "the small text"),
            AnchorStatus::Orphan
        );
    }

    #[test]
    fn resolver_uses_sentence_id_when_quote_unfindable() {
        use scitadel_core::models::sentence_id;
        // Sentence content preserved (same words, different
        // case/whitespace). Quote string is wholly absent from the
        // new text so substring + fuzzy fail; sentence-id rescues.
        let original_sentence = "The Transformer Architecture relies on self-attention.";
        let new_text = "Intro. the   transformer architecture relies on self-attention. Outro.";
        let mut a = Anchor {
            char_range: None,
            // Bypasses substring + fuzzy.
            quote: Some("ZZZ-not-in-new-text-ZZZ".into()),
            sentence_id: Some(sentence_id(original_sentence)),
            ..Anchor::default()
        };
        let s = resolve_anchor(&mut a, new_text);
        assert_eq!(
            s,
            AnchorStatus::Drifted,
            "sentence-id rescue should mark Drifted, got {s:?}"
        );
        let (start, end) = a.char_range.unwrap();
        let resolved: String = new_text.chars().skip(start).take(end - start).collect();
        assert!(
            resolved.contains("transformer architecture"),
            "expected re-anchor to the matching sentence; got {resolved:?}"
        );
    }
}
