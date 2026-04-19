# VHS tape: CLI `scitadel search` + `scitadel history` (#99 / 0.4.0).
#
# Uses arXiv (no auth needed) for a small, real network call. If CI
# turns out to be flaky on the live arXiv lookup, swap to a sqlite3
# seed of papers + a search row (same pattern as
# annotations-in-detail.tape).

Output "tests/vhs/snapshots/cli-search/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 900
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
Sleep 800ms
Type "clear"
Enter
Show

Type `scitadel search "transformer attention" --sources arxiv -n 3`
Sleep 300ms
Enter
Sleep 8s
Screenshot "tests/vhs/snapshots/cli-search/01-search-results.png"

Type "clear"
Enter
Type "scitadel history -n 1"
Sleep 300ms
Enter
Sleep 1s
Screenshot "tests/vhs/snapshots/cli-search/02-history-shows-search.png"
