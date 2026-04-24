# VHS tape: TUI launches, shows default tabs, exits cleanly.
#
# Smoke test — no fixtures, no network. Proves the TUI binary runs
# against a fresh DB and shuts down cleanly on `q`.

Output "tests/vhs/snapshots/tui-launch/run.gif"

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
Type "scitadel init --yes --email demo@example.org --sources arxiv,openalex --db $SCITADEL_DB"
Enter
Sleep 1s
Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2s
Screenshot "tests/vhs/snapshots/tui-launch/01-searches-tab.png"

Tab
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-launch/02-papers-tab.png"

Tab
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-launch/03-questions-tab.png"

# BackTab cycles tabs in reverse — back to Papers (#99 audit gap).
Shift+Tab
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-launch/04-backtab-papers.png"

# `d` / `u` page-scroll bindings.
Type "d"
Sleep 200ms
Type "u"
Sleep 200ms
Screenshot "tests/vhs/snapshots/tui-launch/05-page-scroll.png"

Type "q"
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-launch/06-quit.png"
