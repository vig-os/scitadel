# VHS tape: `scitadel question` subcommand workflow (#99 / 0.4.0).
#
# Deterministic — no network. Creates a question, captures its ID via
# `awk`, attaches search terms, lists, then opens the TUI Questions
# tab to show the row populated.

Output "tests/vhs/snapshots/cli-question-workflow/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 900
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
Type "clear"
Enter
Show

Type `scitadel question create "How do transformers handle long-range dependencies?" --description "compare attention variants vs. recurrence"`
Sleep 300ms
Enter
Sleep 1s
Screenshot "tests/vhs/snapshots/cli-question-workflow/01-create.png"

Type `QID=$(scitadel question list | tail -n 1 | awk '{print $1}')`
Enter
Sleep 300ms

Type `scitadel question add-terms $QID "long-range attention" "sparse transformer" "linformer"`
Sleep 300ms
Enter
Sleep 800ms
Screenshot "tests/vhs/snapshots/cli-question-workflow/02-add-terms.png"

Type "clear"
Enter
Type "scitadel question list"
Sleep 300ms
Enter
Sleep 800ms
Screenshot "tests/vhs/snapshots/cli-question-workflow/03-list.png"

Type "scitadel tui"
Enter
Sleep 1500ms
Tab
Sleep 300ms
Tab
Sleep 500ms
Screenshot "tests/vhs/snapshots/cli-question-workflow/04-tui-questions-tab.png"

Type "q"
Sleep 300ms
