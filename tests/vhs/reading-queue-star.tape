# VHS tape: star toggle in the Papers tab.
#
# Seeds one paper via sqlite3, launches the TUI, switches to Papers,
# presses `s` to star it, captures the `★` prefix.

Output "tests/vhs/snapshots/reading-queue-star/run.gif"

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
Sleep 800ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, created_at, updated_at) VALUES ('p-demo', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, datetime('now'), datetime('now'));"`
Enter
Sleep 400ms
Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2500ms

Tab
Sleep 1500ms
Screenshot "tests/vhs/snapshots/reading-queue-star/01-papers-unstarred.png"

Type "s"
Sleep 1500ms
Screenshot "tests/vhs/snapshots/reading-queue-star/02-papers-starred.png"

Type "s"
Sleep 1500ms
Screenshot "tests/vhs/snapshots/reading-queue-star/03-papers-unstarred-again.png"

Type "q"
Sleep 300ms
