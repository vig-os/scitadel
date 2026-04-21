# VHS tape: Question Dashboard — ranked papers + citation shortlist (#133).
#
# Seeds a question + 3 scored papers, opens the Questions tab,
# presses Enter to open the dashboard, toggles one onto the
# shortlist with `c`.

Output "tests/vhs/snapshots/question-dashboard/run.gif"

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
Type `export SCITADEL_DB=$(mktemp -d)/scitadel.db`
Enter
Type "scitadel init --yes --email demo@example.org --sources arxiv --db $SCITADEL_DB"
Enter
Sleep 1s

# Seed a research question + 3 papers + 3 assessments.
Type `sqlite3 $SCITADEL_DB "INSERT INTO research_questions (id, text, description, created_at, updated_at) VALUES ('q-ml', 'What are the key advances in attention mechanisms?', 'For a lit review section.', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, created_at, updated_at) VALUES ('p-a', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, 'The dominant sequence transduction models are based on complex RNNs or CNNs. We propose the Transformer.', datetime('now'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, created_at, updated_at) VALUES ('p-b', 'Neural Machine Translation by Jointly Learning to Align and Translate', '[\"Bahdanau, D.\"]', 2014, 'Earlier work on attention for sequence-to-sequence models.', datetime('now'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, created_at, updated_at) VALUES ('p-c', 'Long Short-Term Memory', '[\"Hochreiter, S.\"]', 1997, 'A recurrent architecture — foundational but not attention-based.', datetime('now'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO assessments (id, paper_id, question_id, score, reasoning, assessor, created_at) VALUES ('a-1', 'p-a', 'q-ml', 0.95, 'Landmark Transformer paper — core to the question on attention mechanisms.', 'claude-opus-4-7', datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO assessments (id, paper_id, question_id, score, reasoning, assessor, created_at) VALUES ('a-2', 'p-b', 'q-ml', 0.82, 'Earlier but directly relevant: attention for alignment in NMT.', 'claude-opus-4-7', datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO assessments (id, paper_id, question_id, score, reasoning, assessor, created_at) VALUES ('a-3', 'p-c', 'q-ml', 0.25, 'Seminal but pre-attention; include only for historical context.', 'claude-opus-4-7', datetime('now'));"`
Enter
Sleep 300ms

Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2500ms

Tab
Sleep 600ms
Tab
Sleep 1200ms
Screenshot "tests/vhs/snapshots/question-dashboard/01-questions-tab.png"

Enter
Sleep 1500ms
Screenshot "tests/vhs/snapshots/question-dashboard/02-dashboard-open.png"

Type "j"
Sleep 800ms
Screenshot "tests/vhs/snapshots/question-dashboard/03-second-paper-focused.png"

Type "c"
Sleep 800ms
Screenshot "tests/vhs/snapshots/question-dashboard/04-shortlisted.png"

Escape
Sleep 500ms
Screenshot "tests/vhs/snapshots/question-dashboard/05-back-to-questions.png"

Type "q"
Sleep 300ms
