# VHS tape: paper-level notes section + P keybind (#185 PR4).
#
# Seeds a paper plus an existing paper-level note (so the section
# renders even before the user adds anything), then drives the
# `P` keybind to create a second one. The TUI should:
#   1. Show a "Paper-level notes" section above the threaded
#      annotations.
#   2. Open the paper-note prompt on `P` and accept a note.
#   3. Show the new note appended to the paper-level section.

Output "tests/vhs/snapshots/tui-paper-note/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 900
Set Padding 10
Set TypingSpeed 40ms

Hide
Type `export PATH="$PWD/target/release:$HOME/.cargo/bin:$PATH"`
Enter
Type `export PS1="$ "`
Enter
Type `export SCITADEL_DB=$(mktemp -d)/scitadel.db`
Enter
Type "scitadel init --yes --email demo@example.org --sources arxiv --db $SCITADEL_DB"
Enter
Sleep 800ms

Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, full_text, created_at, updated_at) VALUES ('p-pn', 'On Methodology', '[\"Author, A.\"]', 2024, 'A short abstract about methodology.', 'The body of the paper. Methodology is described in section three.', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
# Pre-existing paper-level note from a previous agent session.
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, paper_id, sentence_id, anchor_status, note, tags_json, author, created_at, updated_at) VALUES ('ann-pn-1', 'p-pn', 'paper-note:p-pn', 'ok', 'Solid framing; concerns about sample size.', '[]', 'claude', datetime('now'), datetime('now'));"`
Enter
Sleep 300ms
Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 1500ms

# Tab → Papers, open the seeded paper, enter reader mode.
Tab
Sleep 400ms
Enter
Sleep 800ms
Type "R"
Sleep 1s
Screenshot "tests/vhs/snapshots/tui-paper-note/01-existing-paper-note-section.png"

# Leave reader mode (Esc), then `P` to open the paper-note prompt.
Escape
Sleep 300ms
Type "P"
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-paper-note/02-paper-note-prompt-open.png"

# Type a note body and submit.
Type "Methodology check: rerun ablation with N>1000."
Sleep 300ms
Enter
Sleep 800ms
# Re-enter reader to see the new entry under the paper-level section.
Type "R"
Sleep 1s
Screenshot "tests/vhs/snapshots/tui-paper-note/03-new-paper-note-rendered.png"

Escape
Sleep 200ms
Type "q"
Sleep 200ms
Type "q"
Sleep 400ms
