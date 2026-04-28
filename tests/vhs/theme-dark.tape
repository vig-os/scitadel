# VHS tape: --theme dark renders Dalton Dark (#137).
#
# Mirrors theme-light.tape so the rendered snapshots can be diffed
# side-by-side. CLI flag is forced (highest precedence) so the run is
# deterministic regardless of the CI runner's COLORFGBG.

Output "tests/vhs/snapshots/theme-dark/run.gif"

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
Type "scitadel tui --theme dark"
Enter
Sleep 2500ms
Screenshot "tests/vhs/snapshots/theme-dark/01-searches-dark.png"

Tab
Sleep 800ms
Screenshot "tests/vhs/snapshots/theme-dark/02-papers-dark.png"

Type "q"
Sleep 300ms
