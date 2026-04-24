# VHS tape: scitadel init wizard (non-interactive path).
#
# Exercises `scitadel init --yes`: writes config.toml + migrates the DB.
# Deterministic — no network, no stdin prompts — so CI can rely on it.

Output "tests/vhs/snapshots/init-wizard/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1200
Set Height 520
Set Padding 10
Set TypingSpeed 40ms

Hide
Type `export PATH="$PWD/target/release:$HOME/.cargo/bin:$PATH"`
Enter
Type `export PS1="$ "`
Enter
Type `export SCITADEL_DB=$(mktemp -d)/scitadel.db`
Enter
Type "clear"
Enter
Show

Type "scitadel init --yes --email lars@example.org --sources arxiv,openalex --db $SCITADEL_DB"
Sleep 500ms
Enter
Sleep 2s
Screenshot "tests/vhs/snapshots/init-wizard/01-after-init.png"

Type "cat $(dirname $SCITADEL_DB)/config.toml"
Sleep 300ms
Enter
Sleep 1s
Screenshot "tests/vhs/snapshots/init-wizard/02-config.png"
