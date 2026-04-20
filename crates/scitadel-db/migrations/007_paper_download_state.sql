-- Persist the outcome of paper downloads so the TUI can show a state
-- column ("never tried", "downloaded", "paywalled", "failed") without
-- re-probing on every render and the user can tell at a glance which
-- papers are openable. See #112.

ALTER TABLE papers ADD COLUMN local_path TEXT;
ALTER TABLE papers ADD COLUMN download_status TEXT;
ALTER TABLE papers ADD COLUMN last_attempt_at TEXT;

INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (7, datetime('now'));
