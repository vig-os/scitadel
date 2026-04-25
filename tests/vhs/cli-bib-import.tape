# VHS tape: `scitadel bib import` (#134, 0.6.1 — bib import iter 2).
#
# Demonstrates the new import surface end-to-end: parse a Zotero-shaped
# `.bib`, run the match cascade against an empty DB (everything Created),
# then re-import the same file and watch the alias / bibtex_key cascade
# steps resolve every entry to Unchanged — the round-trip pitfall fix.

Output "tests/vhs/snapshots/cli-bib-import/run.gif"

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
Sleep 800ms
Type "clear"
Enter
Show

# First import: every entry creates a new paper, alias gets recorded,
# notes turn into annotations, files are dropped under --verbose.
Type "scitadel bib import tests/fixtures/zotero-export.bib --reader demo --verbose"
Sleep 300ms
Enter
Sleep 1500ms
Screenshot "tests/vhs/snapshots/cli-bib-import/01-first-import.png"

Type "clear"
Enter

# Second import on the same DB: the round-trip invariant. Every row
# resolves via the alias / bibtex_key step of the match cascade and
# reports Unchanged. Tally line should read "0 created, 0 updated".
Type "scitadel bib import tests/fixtures/zotero-export.bib --reader demo"
Sleep 300ms
Enter
Sleep 1500ms
Screenshot "tests/vhs/snapshots/cli-bib-import/02-round-trip-noop.png"
