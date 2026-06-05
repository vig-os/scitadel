# VHS tape: --theme light renders Dalton Bright (#137).
#
# Forces the CLI flag (highest precedence) so the run is deterministic
# regardless of the CI runner's COLORFGBG. Snapshots before/after the
# Tab key show the light palette across both Searches and Papers tabs.

Output "tests/vhs/snapshots/theme-light/run.gif"

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

# Seed two papers so the table has visible rows in both tabs.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, created_at, updated_at) VALUES ('p-1', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, created_at, updated_at) VALUES ('p-2', 'Deep Residual Learning', '[\"He, K.\"]', 2015, datetime('now'), datetime('now'));"`
Enter
Sleep 200ms

Type "clear"
Enter
Show

# CLI flag overrides anything in env / config.
Type "scitadel tui --theme light"
Enter
Sleep 2500ms
Screenshot "tests/vhs/snapshots/theme-light/01-searches-light.png"

Tab
Sleep 800ms
Screenshot "tests/vhs/snapshots/theme-light/02-papers-light.png"

Type "q"
Sleep 300ms
