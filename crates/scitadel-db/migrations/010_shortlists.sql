-- Per-(question, reader) citation shortlist (#133 Question Dashboard).
-- The shortlist is the "these are the papers I will cite" set curated
-- by the reader while triaging a question's scored papers. It's the
-- input to `bib snapshot <question_id>` (0.6.1, see #134).

CREATE TABLE IF NOT EXISTS shortlist_members (
    question_id TEXT NOT NULL REFERENCES research_questions(id),
    paper_id    TEXT NOT NULL REFERENCES papers(id),
    reader      TEXT NOT NULL,
    added_at    TEXT NOT NULL,
    PRIMARY KEY (question_id, paper_id, reader)
);

CREATE INDEX IF NOT EXISTS idx_shortlist_q_r
    ON shortlist_members(question_id, reader);

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (10, datetime('now'));
