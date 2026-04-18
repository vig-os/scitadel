# VHS tape: OFFLINE indicator in the status bar.
#
# Uses SCITADEL_FORCE_OFFLINE=1 to bypass the real network probe so the
# tape is deterministic.

Output "tests/vhs/snapshots/offline-indicator/run.gif"

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
Type `export SCITADEL_FORCE_OFFLINE=1`
Enter
Type "scitadel init --yes --email demo@example.org --sources arxiv --db $SCITADEL_DB"
Enter
Sleep 800ms
Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2s
Screenshot "tests/vhs/snapshots/offline-indicator/01-offline-badge.png"

Tab
Sleep 500ms
Screenshot "tests/vhs/snapshots/offline-indicator/02-papers-still-works.png"

Type "q"
Sleep 300ms
