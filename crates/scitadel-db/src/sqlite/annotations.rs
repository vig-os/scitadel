//! SQLite-backed repository for annotations (#49 iter 2).
//!
//! Covers CRUD, threaded reply loading, and a minimal anchoring resolver.
//! The resolver currently tries two selectors — exact char range and
//! quote-substring match — and marks orphans for anything else. Fuzzy
//! quote matching and sentence-id lookup are deferred to iter 3 once we
//! have a TUI surfacing orphans to trigger re-anchoring.

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

/// Resolve an anchor against current paper text, updating `status` and
/// (if the quote shifted) `char_range` in place. The resolver tries two
/// selectors today:
///
/// 1. **Position**: `char_range` still hits the same `quote` → `Ok`.
/// 2. **Quote substring**: `text.find(quote)` succeeds → `Drifted`,
///    offsets auto-updated.
///
/// Fuzzy match + sentence-id lookup are deferred to iter 3 (introduces
/// normalization + SHA1 hashing pipeline).
pub fn resolve_anchor(anchor: &mut Anchor, text: &str) -> AnchorStatus {
    // Attempt 1: current offsets still match the quote exactly.
    if let (Some((start, end)), Some(quote)) = (anchor.char_range, anchor.quote.as_ref()) {
        let candidate: String = text.chars().skip(start).take(end - start).collect();
        if &candidate == quote {
            anchor.status = AnchorStatus::Ok;
            return AnchorStatus::Ok;
        }
    }

    // Attempt 2: find the quote anywhere in the text.
    if let Some(quote) = anchor.quote.as_ref()
        && let Some(byte_pos) = text.find(quote.as_str())
    {
        let start_char = text[..byte_pos].chars().count();
        let end_char = start_char + quote.chars().count();
        anchor.char_range = Some((start_char, end_char));
        anchor.status = AnchorStatus::Drifted;
        return AnchorStatus::Drifted;
    }

    anchor.status = AnchorStatus::Orphan;
    AnchorStatus::Orphan
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
}
