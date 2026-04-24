# VHS tape: Queue tab aggregates starred papers across searches (#48).
#
# Seeds 3 papers in separate searches, stars 2, switches to the new
# Queue tab, confirms the aggregator shows only the starred pair.

Output "tests/vhs/snapshots/queue-tab/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 800
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

# Seed 3 papers with distinct titles.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, created_at, updated_at) VALUES ('p-a', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, created_at, updated_at) VALUES ('p-b', 'Deep Residual Learning', '[\"He, K.\"]', 2015, datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, created_at, updated_at) VALUES ('p-c', 'GPT-3 Language Models', '[\"Brown, T.\"]', 2020, datetime('now'), datetime('now'));"`
Enter
Sleep 200ms

# Star p-a and p-c, leave p-b unstarred. Reader defaults to $USER.
Type `sqlite3 $SCITADEL_DB "INSERT INTO paper_state (paper_id, reader, starred, to_read, updated_at) VALUES ('p-a', '$USER', 1, 0, datetime('now', '-2 minutes'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO paper_state (paper_id, reader, starred, to_read, updated_at) VALUES ('p-c', '$USER', 1, 0, datetime('now'));"`
Enter
Sleep 200ms

Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2500ms

# Tab → Papers → Questions → Queue. The Queue tab lives after the
# existing 3; cycle forward three times.
Tab
Sleep 600ms
Tab
Sleep 600ms
Tab
Sleep 1500ms
Screenshot "tests/vhs/snapshots/queue-tab/01-queue-tab-open.png"

# j to move the cursor down one row (p-c → p-a).
Type "j"
Sleep 500ms
Screenshot "tests/vhs/snapshots/queue-tab/02-second-paper-selected.png"

# s to unstar the focused paper (p-a). List should shrink to 1 entry.
Type "s"
Sleep 1000ms
Screenshot "tests/vhs/snapshots/queue-tab/03-after-unstar.png"

Type "q"
Sleep 300ms
