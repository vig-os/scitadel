# VHS tape: CLI `scitadel auth` against the credential store (#212).
#
# Walks the OpenAlex login (API key, now required, + the optional
# polite-pool email), `auth status` naming the store in use, and logout.
# The store is forced to the 0600 file backend under a temp dir, so the
# tape is deterministic on any runner and never touches a real keyring.

Output "tests/vhs/snapshots/cli-auth/run.gif"

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
Type `unset $(compgen -e | grep '^SCITADEL_')`
Enter
Type `export SCITADEL_CREDENTIAL_BACKEND=file`
Enter
Type `export SCITADEL_CREDENTIALS_FILE=$(mktemp -d)/credentials.toml`
Enter
Type "clear"
Enter
Show

Type "scitadel auth login openalex"
Sleep 300ms
Enter
Sleep 800ms
# The API key is read without echo.
Type "oa-demo-key-123"
Enter
Sleep 500ms
Type "demo@example.org"
Enter
Sleep 800ms
Screenshot "tests/vhs/snapshots/cli-auth/01-login-openalex.png"

Type "clear"
Enter
Type "scitadel auth status"
Sleep 300ms
Enter
Sleep 1s
Screenshot "tests/vhs/snapshots/cli-auth/02-status-names-backend.png"

Type "clear"
Enter
Type "scitadel auth logout openalex && scitadel auth status | head -8"
Sleep 300ms
Enter
Sleep 1s
Screenshot "tests/vhs/snapshots/cli-auth/03-logout.png"
