# VHS tape: runtime `T` toggle flips dark → light mid-session (#175).
#
# Launches with `--theme dark` (deterministic regardless of the CI
# runner's COLORFGBG), captures the dark palette, presses uppercase T
# to cycle, and captures the light palette + the `theme: dalton-bright`
# status-bar toast that fires for ~30 frames after the toggle.

Output "tests/vhs/snapshots/theme-toggle/run.gif"

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

# Seed a paper so the table has at least one visible row — the toggle
# is recognisable even on an empty screen via the status bar, but the
# selected-row background is the strongest visual signal of a swap.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, created_at, updated_at) VALUES ('p-1', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, datetime('now'), datetime('now'));"`
Enter
Sleep 200ms

Type "clear"
Enter
Show

# Start in the dark palette so the post-toggle frame is unambiguous.
Type "scitadel tui --theme dark"
Enter
Sleep 2500ms
Screenshot "tests/vhs/snapshots/theme-toggle/01-before-dark.png"

# Cycle to dalton-bright. Status bar should flash `theme: dalton-bright`
# for ~30 frames; the screenshot below is captured well within that
# window so the toast is still on screen.
Type "T"
Sleep 500ms
Screenshot "tests/vhs/snapshots/theme-toggle/02-after-light.png"

# Cycle back to dalton-dark so the wrap-around is also exercised
# visually, not just in the unit tests.
Type "T"
Sleep 500ms
Screenshot "tests/vhs/snapshots/theme-toggle/03-wrap-back-dark.png"

Type "q"
Sleep 300ms
