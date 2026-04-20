-- Singleton row tracking the TUI's current selection (#122) so an
-- agent in an adjacent pane (the recommended 2-pane workflow) can ask
-- "what is the user looking at right now?" without the user having to
-- paste IDs. Last-writer-wins if multiple TUIs run concurrently.

CREATE TABLE tui_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    tab TEXT NOT NULL,
    paper_id TEXT,
    search_id TEXT,
    question_id TEXT,
    annotation_id TEXT,
    updated_at TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (8, datetime('now'));
