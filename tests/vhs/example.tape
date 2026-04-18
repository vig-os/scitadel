# Smoke-test tape — proves the VHS harness itself works.
#
# Runs `scitadel --help` in a fresh shell with SCITADEL_DB pointed at a
# disposable temp DB. Any failure here means the VHS install or the CLI
# wrapper is broken, not that a feature regressed.

Output "tests/vhs/snapshots/example/help.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1200
Set Height 600
Set Padding 10

Hide
Type "export SCITADEL_DB=$(mktemp -d)/scitadel.db"
Enter
Show

Type "scitadel --help"
Sleep 500ms
Enter
Sleep 2s
Screenshot "tests/vhs/snapshots/example/01-help.png"

Type "scitadel --version"
Sleep 500ms
Enter
Sleep 1s
Screenshot "tests/vhs/snapshots/example/02-version.png"
