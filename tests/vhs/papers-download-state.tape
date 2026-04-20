# VHS tape: Papers table renders the download-state column (#112).
#
# Seeds 4 papers, each in a different download_status state
# (none / downloaded / paywall / failed) so the symbol column
# renders all four glyphs side-by-side: blank, ✓, ⊘, ✗.

Output "tests/vhs/snapshots/papers-download-state/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 600
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

# Seed 4 papers with distinct download states.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, abstract, created_at, updated_at) VALUES ('p-none', 'Paper Never Tried', '[\"Alice\"]', '', datetime('now', '-4 hours'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, abstract, local_path, download_status, last_attempt_at, created_at, updated_at) VALUES ('p-ok', 'Paper Successfully Downloaded', '[\"Bob\"]', '', '/tmp/p-ok.pdf', 'downloaded', datetime('now'), datetime('now', '-3 hours'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, abstract, local_path, download_status, last_attempt_at, created_at, updated_at) VALUES ('p-pay', 'Paper Behind Paywall', '[\"Carol\"]', '', '/tmp/p-pay.html', 'paywall', datetime('now'), datetime('now', '-2 hours'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, abstract, download_status, last_attempt_at, created_at, updated_at) VALUES ('p-fail', 'Paper Download Failed', '[\"Dave\"]', '', 'failed', datetime('now'), datetime('now', '-1 hours'), datetime('now'));"`
Enter
Sleep 200ms

Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 1500ms

# Tab to Papers tab — table should show 4 rows with blank/✓/⊘/✗ symbols.
Tab
Sleep 500ms
Screenshot "tests/vhs/snapshots/papers-download-state/01-papers-table.png"

Type "q"
Sleep 300ms
Screenshot "tests/vhs/snapshots/papers-download-state/02-quit.png"
