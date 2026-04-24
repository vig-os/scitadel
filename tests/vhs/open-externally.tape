# VHS tape: 'O' keybind opens local PDF in OS viewer (#144).
#
# We deliberately seed a paper WITHOUT a local_path so pressing O in
# the Detail overlay surfaces the "no local file — press D to download
# first" failure in the task panel. This is the only path safe to
# render in CI; the success path would actually launch a viewer and
# steal focus from the headless terminal session.

Output "tests/vhs/snapshots/open-externally/run.gif"

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

# Seed one paper. local_path is intentionally NULL so O surfaces the
# "no local file" failure instead of launching a viewer.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, created_at, updated_at) VALUES ('p-x', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, datetime('now'), datetime('now'));"`
Enter
Sleep 200ms

Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2500ms

# Tab to Papers, open the row.
Tab
Sleep 500ms
Sleep 500ms
Screenshot "tests/vhs/snapshots/open-externally/01-papers-tab.png"

Enter
Sleep 1500ms
Screenshot "tests/vhs/snapshots/open-externally/02-detail-open.png"

# Press O — paper has no local file, so we expect a Failed task row.
Type "O"
Sleep 1500ms
Screenshot "tests/vhs/snapshots/open-externally/03-no-local-file-failure.png"

Type "q"
Sleep 300ms
Type "q"
Sleep 300ms
