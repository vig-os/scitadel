---
type: issue
state: closed
created: 2026-10-02T23:43:58Z
updated: 2026-10-03T00:44:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/276
comments: 1
labels: bug, effort:small, security, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:41.229Z
---

# [Issue 276]: [security(adapters): the OpenAlex API key leaks into error strings, reaching the TUI and the MCP agent](https://github.com/vig-os/scitadel/issues/276)

## Problem

The OpenAlex API key is passed as a **query parameter**, so it is part of the request URL:

\`\`\`
https://api.openalex.org/works/doi:10.1038/...?api_key=sk-...
\`\`\`

\`reqwest::Error\`'s \`Display\` interpolates the URL, and \`FetchError::Status\` carries the URL too. Anywhere a download or search error is formatted, that string carries the key.

Where it surfaces:

- \`crates/scitadel-tui\` task panel — an error is rendered next to the paper title, so the key lands on screen and in any terminal scrollback or recording.
- the MCP \`download_paper\` / \`search\` tool return value — the string goes straight back to the calling agent, which may log it, echo it into a conversation, or write it to a transcript.

This is pre-existing, not introduced by the acquisition work. It is the same class as #248 (a credential reaching somewhere it should not) and was found while building #252's paced download path, which surfaced it because \`FetchError\` makes the URL explicit.

## Why it is easy to miss

Nothing logs a "key" — the key is simply *in a URL*, and URLs are treated as non-sensitive everywhere. So no amount of auditing "places we print credentials" finds it. A grep for \`api_key\` in log statements comes back clean.

## Proposal

1. **Redact at the boundary, not at the source.** Add \`FetchError::redacted()\` (or a \`Display\` that already redacts) and make it the form used by anything user- or agent-facing. Redact query parameters whose name looks like a credential (\`api_key\`, \`token\`, \`access_token\`, \`key\`, \`apikey\`) plus any \`Authorization\`/\`Cookie\` header value.
2. **Fail the test if it regresses.** A unit test that builds an error containing \`?api_key=sk-secret\` and asserts the rendered string does not contain \`sk-secret\` — otherwise this comes straight back the next time someone adds a \`Display\`.
3. **Consider not putting the key in the URL at all.** OpenAlex accepts the \`api_key\` query parameter; if it also accepts a header, that removes the exposure at source. Worth checking against their docs — if a header is supported it is strictly better than redacting every copy of the string.

## Acceptance

- [ ] No \`Display\`/\`Debug\` path reachable from the TUI or an MCP tool return can render a credential, whether it arrived as a query parameter or a header.
- [ ] The redaction is covered by a test that fails on regression.
- [ ] The OpenAlex key is no longer in a URL, if a header form exists.

## Related

#248 (argv leak), #252 (found while building the paced download path).
---

# [Comment #1]() by [gerchowl]()

_Posted on October 3, 2026 at 12:44 AM_

Fixed by #277.

Two leak paths, not one.

**The URL field.** Every URL-bearing `FetchError` variant now stores a redacted URL — query parameters whose name looks credential-shaped, authority credentials, fragment parameters. Redaction happens where the value is *stored*, not where it is printed: `FetchError` derives `Debug`, so redacting in `Display` alone would leave the secret in the struct.

**The source field.** `reqwest::Error` embeds the request URL in both its `Display` and its `Debug`, so rendering `{source}` hands the key back regardless of how `url` is treated. The source is wrapped in `TransportSource`, whose formatting is a URL-free classification, with the typed error still reachable via `error()`.

The second path is now **structurally impossible** rather than guarded — storing a bare `reqwest::Error` no longer compiles.

The end-to-end test makes a real request with `?api_key=…` to a closed port and asserts the credential is absent from both `Display` and `Debug`. It earned its place: it caught the `{source}` path after the first round of fixes had already gone green on `Display` alone.

The third item in the issue — moving the key out of the URL entirely, if OpenAlex accepts a header — is **not** done. That needs checking against their current API docs, and inventing the answer was the wrong move. The key is still in the query string on the wire; it simply cannot escape into a message now."

