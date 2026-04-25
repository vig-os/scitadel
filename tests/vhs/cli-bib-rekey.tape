# VHS tape: `scitadel bib rekey` (#134, 0.6.1 — bib rekey iter 2).
#
# Demonstrates the rekey escape hatch: import the fixture so we have
# a paper with an algorithmic key, then rekey it to an explicit
# value. The alias-preservation breadcrumb ensures the old key keeps
# resolving so manuscripts that still cite by it don't break.

Output "tests/vhs/snapshots/cli-bib-rekey/run.gif"

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
Type "scitadel init --yes --email demo@example.org --sources arxiv,openalex --db $SCITADEL_DB"
Enter
Sleep 600ms
Type "scitadel bib import tests/fixtures/zotero-export.bib --reader demo > /dev/null"
Enter
Sleep 1500ms
Type `PID=$(scitadel bib import tests/fixtures/zotero-export.bib --reader demo | awk '/^  unchanged/ {print $2; exit}')`
Enter
Sleep 1500ms
Type "clear"
Enter
Show

# Rekey to an explicit, more memorable value. Output shows the old →
# new mapping plus the alias-preservation breadcrumb.
Type `scitadel bib rekey "$PID" --key memorable-key --reader demo`
Sleep 300ms
Enter
Sleep 1500ms
Screenshot "tests/vhs/snapshots/cli-bib-rekey/01-rekey-explicit.png"

Type "clear"
Enter

# Collision: try to rekey a different paper to the same key. Fails
# loudly with the owner-paper id so the user knows where to look.
Type `OTHER=$(scitadel bib import tests/fixtures/zotero-export.bib --reader demo | awk -v p="$PID" '/^  unchanged/ && $2 != p {print $2; exit}')`
Enter
Sleep 1500ms
Type `scitadel bib rekey "$OTHER" --key memorable-key --reader demo`
Sleep 300ms
Enter
Sleep 1500ms
Screenshot "tests/vhs/snapshots/cli-bib-rekey/02-collision-rejection.png"
