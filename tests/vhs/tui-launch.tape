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
Type `export PATH="$HOME/.cargo/bin:$PWD/target/release:$PATH"`
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

Type "q"
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-launch/04-quit.png"
