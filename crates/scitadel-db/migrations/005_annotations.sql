-- Migration 005: annotations (highlights + threaded notes) and read receipts.
--
-- Multi-selector anchoring follows the W3C Web Annotation model:
-- position (char_start/end), quote + context (quote/prefix/suffix),
-- and a sentence hash (sentence_id). Any of them may be NULL; the
-- resolver tries them in order and marks drift or orphan status.
--
-- parent_id is self-referential: NULL = anchored root, non-NULL = reply.
-- Replies inherit their anchor from the root; selector columns on
-- replies are expected to be NULL.
--
-- `deleted_at` is a soft-delete tombstone so that threads are preserved
-- even when their root is deleted.

CREATE TABLE IF NOT EXISTS annotations (
    id             TEXT PRIMARY KEY,
    parent_id      TEXT REFERENCES annotations(id),
    paper_id       TEXT NOT NULL REFERENCES papers(id),
    question_id    TEXT REFERENCES research_questions(id),

    -- Multi-selector anchor (roots only; replies inherit from root).
    char_start     INTEGER,
    char_end       INTEGER,
    quote          TEXT,
    prefix         TEXT,
    suffix         TEXT,
    sentence_id    TEXT,
    source_version TEXT,
    anchor_status  TEXT CHECK(anchor_status IN ('ok', 'drifted', 'orphan')),

    note           TEXT NOT NULL DEFAULT '',
    color          TEXT,
    tags_json      TEXT NOT NULL DEFAULT '[]',
    author         TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    deleted_at     TEXT
);

CREATE INDEX IF NOT EXISTS idx_annotations_paper    ON annotations(paper_id);
CREATE INDEX IF NOT EXISTS idx_annotations_parent   ON annotations(parent_id);
CREATE INDEX IF NOT EXISTS idx_annotations_question ON annotations(question_id);
CREATE INDEX IF NOT EXISTS idx_annotations_author   ON annotations(author);

CREATE TABLE IF NOT EXISTS annotation_reads (
    annotation_id TEXT NOT NULL REFERENCES annotations(id),
    reader        TEXT NOT NULL,
    seen_at       TEXT NOT NULL,
    PRIMARY KEY (annotation_id, reader)
);

CREATE INDEX IF NOT EXISTS idx_annotation_reads_reader ON annotation_reads(reader);

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (5, datetime('now'));
