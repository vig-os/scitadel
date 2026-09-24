---
type: issue
state: closed
created: 2026-09-23T23:57:36Z
updated: 2026-09-24T09:22:18Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/212
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-24T11:57:14.288Z
---

# [Issue 212]: [[BUG] OpenAlex unusable on Linux: no API-key support, macOS-only credential store, HTTP errors reported as 0 results](https://github.com/vig-os/scitadel/issues/212)

### Description
On Linux (NixOS), scitadel 0.7.0 can't use OpenAlex anymore. Three separate defects stack up:

1. **No OpenAlex API-key support.** The adapter only sends `mailto=<email>` (`crates/scitadel-adapters/src/openalex.rs`, `fetch_from`). OpenAlex now meters keyless requests against a shared per-IP daily budget and returns **429** once it's spent, with or without `mailto`. A key (`api_key=<key>`) is required. The `config.openalex.api_key` field in `scitadel-cli/src/commands.rs` actually holds the *email*, which is misleading.
2. **The credential store only works on macOS.** `scitadel-core/src/credentials.rs` shells out to the macOS `security` CLI, so `scitadel auth login openalex` fails on Linux with `Error: failed to run security CLI: No such file or directory (os error 2)`.
3. **HTTP errors are silently reported as `0 results`.** The adapter returns `Err(HTTP 429 …)`, but `scitadel search` prints `[+] openalex: 0 results (58ms)`. Users (and agents) can't tell "nothing found" from "source down", so literature sweeps silently lose a whole source.

### Steps to Reproduce
1. On Linux, after the per-IP OpenAlex budget is spent: `scitadel search "DOTA lutetium" -s openalex -n 5` prints `openalex: 0 results`.
2. `curl "https://api.openalex.org/works?search=DOTA&per-page=1&mailto=me@example.org"` returns 429 (`Insufficient budget. This request has no API key…`).
3. The same with `&api_key=<valid key>` returns 200 (3720 hits).
4. `scitadel auth login openalex` fails with `failed to run security CLI: No such file or directory`.

### Expected Behavior
- `openalex.api_key` (keychain) / `SCITADEL_OPENALEX_API_KEY` (env) / `.scitadel/config.toml` are sent as `api_key=`; the email stays the polite-pool `mailto` (`SCITADEL_OPENALEX_EMAIL`).
- `auth login` works on Linux (libsecret/Secret Service via `secret-tool`, or a 0600 file fallback), and `auth status` reports the backend.
- An adapter HTTP/network error is shown as an error (e.g. `[!] openalex: HTTP 429 …`) and reflected in the search run record, never as `0 results`.

### Actual Behavior
See above: 429s become `0 results`, there's no way to pass a key, and `auth login` crashes on Linux.

### Environment
scitadel 0.7.0 (nix store build), NixOS (Linux 6.18), found while running scitadel from an agentic literature pipeline (raid project).
