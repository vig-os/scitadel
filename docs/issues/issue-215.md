---
type: issue
state: open
created: 2026-09-24T09:44:13Z
updated: 2026-09-24T09:44:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/215
comments: 0
labels: security
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-24T11:57:13.944Z
---

# [Issue 215]: [Clear open Dependabot security advisories (openssl, rustls-webpki, rmcp, lru, rand, rpassword)](https://github.com/vig-os/scitadel/issues/215)

20 open Dependabot alerts on `main` (9 high), all in `Cargo.lock`.

| Crate | Fixed in | Path |
| --- | --- | --- |
| openssl (8 advisories) | ≥ 0.10.80 | lockfile bump |
| rustls-webpki (4) | ≥ 0.103.13 | lockfile bump |
| rand 0.9 / 0.10 | 0.9.3 / 0.10.1 | lockfile bump |
| rpassword | 7.5.0 | lockfile bump |
| rmcp (4) | ≥ 2.1.0 | API upgrade 0.17 → 3.x (scitadel only runs the stdio server, so the HTTP/OAuth advisories are not reachable today, but the crate is pinned far behind) |
| lru | ≥ 0.16.3 | pulled in by ratatui 0.29; needs ratatui 0.30 (lru 0.18) + crossterm 0.29 |

Three PRs, all targeting `dev` for the next release.
