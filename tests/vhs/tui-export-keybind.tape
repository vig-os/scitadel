# VHS tape: TUI export keybind — `E` on Question Dashboard (#135 sub-feature B).
#
# Seeds a question + a single shortlisted paper, opens the Question
# Dashboard, presses `E`, accepts the slug-derived default path, and
# screenshots the success toast.

Output "tests/vhs/snapshots/tui-export-keybind/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1600
Set Height 900
Set Padding 10
Set TypingSpeed 40ms

Hide
Type `export PATH="$PWD/target/release:$HOME/.cargo/bin:$PATH"`
Enter
Type `export PS1="$ "`
Enter
Type `export TMPDIR=$(mktemp -d)`
Enter
Type `export SCITADEL_DB=$TMPDIR/scitadel.db`
Enter
Type `cd $TMPDIR`
Enter
Type "scitadel init --yes --email demo@example.org --sources arxiv --db $SCITADEL_DB"
Enter
Sleep 1s

# Seed: 1 question + 1 paper + 1 assessment + 1 shortlist row.
Type `sqlite3 $SCITADEL_DB "INSERT INTO research_questions (id, text, description, created_at, updated_at) VALUES ('q-attn', 'What is the role of attention?', 'For a lit review section.', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, created_at, updated_at) VALUES ('p-a', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, 'The dominant sequence transduction models are based on complex RNNs or CNNs.', datetime('now'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO assessments (id, paper_id, question_id, score, reasoning, assessor, created_at) VALUES ('a-1', 'p-a', 'q-attn', 0.95, 'Landmark Transformer paper.', 'claude-opus-4-7', datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO shortlist (question_id, paper_id, reader, added_at, updated_at) VALUES ('q-attn', 'p-a', '$USER', datetime('now'), datetime('now'));"`
Enter
Sleep 300ms

Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2500ms

# Tab → Tab → Tab to land on Questions.
Tab
Sleep 400ms
Tab
Sleep 400ms
Tab
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-export-keybind/01-questions-tab.png"

Enter
Sleep 1200ms
Screenshot "tests/vhs/snapshots/tui-export-keybind/02-dashboard-open.png"

# Press `E` (uppercase) to open the export-path prompt.
Type "E"
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-export-keybind/03-export-prompt.png"

# Accept the slug-derived default (`what-is-the-role-of-attention.bib`).
Enter
Sleep 1000ms
Screenshot "tests/vhs/snapshots/tui-export-keybind/04-export-success-toast.png"

Type "q"
Sleep 200ms
Type "q"
Sleep 200ms
