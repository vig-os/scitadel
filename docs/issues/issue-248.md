---
type: issue
state: closed
created: 2026-09-30T00:11:50Z
updated: 2026-10-02T23:19:19Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/248
comments: 1
labels: bug, effort:small, security, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:47.707Z
---

# [Issue 248]: [security(credentials): macOS keychain store passes the secret on argv (visible in ps)](https://github.com/vig-os/scitadel/issues/248)

## Problem
`crates/scitadel-core/src/credentials.rs:~270` stores a secret with `security add-generic-password … -w <value> -U`. The value sits in the process's argv for as long as the command runs, so any local user can read it with `ps` or through process accounting. The read path (`find-generic-password -w`, L251) is fine; it prints to stdout.

## Fix
- Store without putting the value in argv. Either pass `-w` last with no value (`security` then prompts, and we feed the value on stdin), or use the Security framework directly (`security-framework` crate) instead of the CLI.
- Wrap secret values in a `Secret` newtype: redacting `Debug`/`Display`, zeroize on drop, no `Serialize`.
- Test: a store and retrieve round-trip on macOS CI. A unit test asserts that the built `Command` args never contain the value.

Found in the #247 security review. #247 will add publisher TDM keys to this same store, so this should land first.

Refs: #213, #247
---

# [Comment #1]() by [gerchowl]()

_Posted on October 2, 2026 at 11:19 PM_

Fixed by #264 (merged into `dev`).

**The fix proposed in this issue would have been worse than the bug.** It suggested passing a bare `-w` so that `security` prompts and the value arrives on stdin. It does not prompt. Apple's SecurityTool source (`keychain_add.c`) parses with `getopt(argc, argv, "a:c:C:D:G:j:l:s:p:w:UAT:")` and does `passwordData = optarg` for `-w`/`-p`, with no prompt fallback — so a bare trailing `-w` stores an **empty password**, and `-w -U` consumes `-U` as the password.

Only calling Security.framework keeps the value out of argv, so get/store/delete now use `security-framework` (macOS-gated) instead of the CLI.

Also landed: a `Secret` newtype (redacted `Debug`/`Display`, zeroized on drop, deliberately **not** `Serialize`), and `Backend::MacosKeychain` is now `cfg(target_os = "macos")` so a shared config naming it degrades to auto-detection off macOS instead of selecting a backend that cannot work.

Verified with a fake `secret-tool` that records both argv and stdin; the test fails with `the secret reached the process argv, readable via ps` if the secret is ever put back on argv.

**Migration note for users:** an item written by an older scitadel carries an ACL trusting `/usr/bin/security`, so the first read by scitadel itself may raise a one-time Keychain prompt. Re-running `scitadel auth login <source>` rewrites the item and clears it. Items are found either way.

