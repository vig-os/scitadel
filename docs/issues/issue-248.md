---
type: issue
state: open
created: 2026-09-30T00:11:50Z
updated: 2026-09-30T00:11:50Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/248
comments: 0
labels: bug, effort:small, security, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:23.208Z
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
