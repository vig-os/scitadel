---
type: issue
state: open
created: 2026-09-28T12:35:29Z
updated: 2026-09-28T23:14:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/230
comments: 1
labels: feature, effort:large, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:24.390Z
---

# [Issue 230]: [feature: credentialed + human-assisted paywall access (TDM tokens, session broker, live TUI queue, agent-drivable)](https://github.com/vig-os/scitadel/issues/230)

## Motivation

`scitadel-adapters` already *detects* the wall — `AccessStatus::Paywall` in
`crates/scitadel-adapters/src/download.rs` — but has no route past it. Every
consumer therefore has to build its own credentialed fetching, and they all
build it slightly differently and slightly wrong.

Concrete consumer pressure: the `raid` project (radiometal radiopharmaceutical
datasets) has **212 unique paywalled DOIs out of 587 known**, and its corpus is
now its primary deliverable, so those 212 are directly blocking. Its measured
breakdown by publisher:

| publisher | paywalled | priority-1 |
|---|---:|---:|
| ACS (10.1021) | 65 | 20 |
| Elsevier (10.1016/06/53) | 41 | 12 |
| Taylor & Francis (10.1080) | 25 | 8 |
| RSC (10.1039) | 25 | 5 |
| Springer Nature (10.1007/38/86) | 16 | 7 |
| SNMMI / J Nucl Med (10.2967) | 13 | 9 |
| Wiley (10.1002/1111) | 10 | 2 |
| De Gruyter, other | 17 | 7 |

Note the shape: the *generic* TDM trio (Elsevier/Springer/Wiley) is only 67 of
212. For a chemistry corpus ACS and RSC dominate, and SNMMI has the highest
priority-1 density of any publisher. Any design that assumes "the big three
cover it" is wrong for subject-specialist corpora — which is most of them.

Credentials and sessions belong in the literature layer, once, not in each
consumer.

## Proposed approach

An **access ladder** resolved per DOI, cheapest and most-sanctioned first:

1. **Open access** — existing behaviour (OpenAlex / Unpaywall / arXiv / OSTI).
2. **Publisher TDM API** — where a token exists (Elsevier `X-ELS-APIKey` +
   `X-ELS-Insttoken`, Springer `api_key`, Wiley `Wiley-TDM-Client-Token`, and
   whatever ACS/RSC turn out to broker). Fully scriptable, no session needed.
3. **Institutional IP** — if the host is on the subscribing network/VPN, the
   plain request already works and *no credential exists in the flow at all*.
   Cleanest tier; worth detecting explicitly rather than stumbling into.
4. **Human-assisted session** — the interesting one, and the subject of the
   rest of this issue.
5. **`needs_manual`** — surfaced as a short actionable list, never a silent gap.

### Tier 4: human-assisted session

The legitimate framing is *"a researcher opens a paper they have access to and
reads it"* — scitadel just reuses that same authenticated session to store the
artefact it was going to render anyway. That is materially different from
scraping, **provided it keeps human pacing** (see Pitfalls — this is the part
that can go badly wrong).

Mechanics:

- A **session broker** owns browser contexts per publisher domain and knows,
  per publisher, whether a live authenticated session exists and when it
  looks stale.
- When the queue hits a DOI whose publisher has no live session, scitadel
  emits an **auth request**: a URL for the human to open (the DOI resolver
  link, or the publisher's institutional-login entry point).
- The human completes SSO/2FA in a real browser. Scitadel **probes** for
  completion — a cheap per-publisher request whose success means "we are in" —
  rather than guessing from cookie names, which differ per publisher.
- On success the broker marks that publisher authenticated and **drains that
  publisher's queue slice**, rate-limited, until it stalls or empties.
- On a mid-drain 401/403/redirect-to-login it re-raises the auth request for
  that publisher and pauses that slice only — other publishers keep draining.

### Tier 4 UX: live TUI

`scitadel-tui` already exists, so this is a view, not a new app. Roughly:

```
 queue: 212 paywalled            drained: 38    session: 3 publishers live

 publisher        queued  state            action
 ACS                  65  ● authed         draining  (12/65, ~20s each)
 Elsevier             41  ○ needs login    [open login ↗]
 Springer             16  ● authed         idle
 T&F                  25  ⚠ no TDM route   [open login ↗]
 SNMMI                13  ○ needs login    [open login ↗]
```

- Rows carry **OSC 8 hyperlinks** so the login link is clickable in a modern
  terminal, with a plain-URL fallback when the terminal does not support it.
- Live progress per publisher; pause/resume per slice; a visible rate limit.
- The human's only job is the click-and-authenticate moments.

### Agent-drivable surface (MCP)

This is what makes it usable in an agent loop, with a human doing only auth:

- `fetch_queue_add(dois[])` / `fetch_queue_status()`
- `auth_pending()` → `[{publisher, login_url, queued_count, reason}]` — the
  structured "what do you need from the human *right now*" list, so an agent
  can surface exactly that and nothing else
- `fetch_drain(publisher?, max_items?)` → progress + result summary
- `auth_status()` → per-publisher `{has_credential: bool, fingerprint, live_session: bool, expires_hint}`
  — **fingerprints only, never values**

The loop becomes: agent queues DOIs → agent reads `auth_pending()` → agent
asks the human to click N links → human authenticates → agent calls
`fetch_drain()` → repeats. The human is in the loop only where a human is
actually required.

## What already exists

- `crates/scitadel-adapters/src/download.rs` — `AccessStatus::{FullText,
  Abstract, Paywall, Unknown}`, `detect_access_status()`, `download_paper()`.
  The paywall trigger is already there and unused.
- `crates/scitadel-tui/` — app/views/widgets/tasks scaffolding.
- `crates/scitadel-core/src/config.rs` — existing auth config shape
  (`OpenAlexAuth`) to generalise from.
- `crates/scitadel-mcp/` — the agent surface to extend.

## Scope

**P0 — credentialed routes (no browser)**
- [ ] Generalise config to per-publisher credentials; resolve DOI prefix → publisher → route
- [ ] Tier 2 for Elsevier / Springer / Wiley (documented header shapes)
- [ ] Tier 3 institutional-IP detection, reported explicitly
- [ ] `auth_status()` returning fingerprints only; a credential value must never be returnable through any API or log
- [ ] Rate limiter + per-publisher daily cap, on by default

**P1 — human-assisted session**
- [ ] Session broker with per-publisher liveness + per-publisher auth probe
- [ ] `auth_pending()` / `fetch_drain()` MCP verbs
- [ ] Re-auth on mid-drain 401/403 without killing other slices
- [ ] Artefact store records route used, retrieval time, and `sha256`

**P2 — TUI + polish**
- [ ] Live queue view with OSC 8 login links and plain fallback
- [ ] Per-publisher pause/resume, visible rate limit and cap
- [ ] ACS / RSC / SNMMI routes once their sanctioned mechanism is confirmed

## Pitfalls

1. **This can get an institution's access cut off — for everyone.** The risk
   is not "a user gets a warning". Publishers detect systematic downloading and
   suspend the *subscribing institution*. Rate limiting, per-publisher daily
   caps and human pacing must be **defaults that are hard to disable**, not
   flags. The consumer's existing discipline (≥20 s/paper, one tab, queued DOIs
   only, never crawl) is the right baseline. A TUI that *feels* like a bulk
   downloader will get used like one — the UI should show the rate limit as a
   feature, not hide it.
2. **Session cookies are bearer credentials.** Anything driving a CDP session
   can read them. If the goal is "an agent can drive this without ever holding
   credentials", that needs real process isolation (separate uid / daemon with
   a narrow API), not convention. Worth deciding explicitly: is the boundary
   *enforced* or *disciplined*? Document whichever is chosen, honestly.
3. **Never store the credential where an agent reads it.** Prefer OS keyring or
   `systemd` credentials over a dotfile. Every surface returns fingerprints.
4. **Auth-completion detection is per-publisher and fragile.** Cookie-name
   heuristics will rot. Use a cheap authenticated probe per publisher and treat
   its response as the only truth.
5. **Do not redistribute paywalled full text.** The cache is a local research
   artefact. Consumers with licence policies (raid buckets records A–D) need
   scitadel to record provenance and access route per artefact so downstream
   licensing stays decidable.
6. **Headless will not survive SSO/2FA.** Tier 4 needs a real, visible browser
   for the login step. Design for attach-to-existing-browser, not launch-headless.
7. **Mid-drain expiry** is the normal case, not the exception.
8. **Don't assume the big three.** Route coverage should be *reported* per
   corpus (see the table above), so a user learns early that their subject area
   is 30% covered rather than discovering it at paper 150.

## Acceptance

- [ ] A DOI list spanning ≥4 publishers resolves each to a route and reports coverage before fetching
- [ ] Tier 2 works end to end for at least one publisher with a real token
- [ ] Tier 4: a queue containing ≥2 unauthenticated publishers emits per-publisher auth requests, and drains after the human authenticates, without restarting the process
- [ ] A credential value cannot be obtained through any MCP verb, CLI output, log line or error message — asserted by a test
- [ ] Rate limit and daily cap are enforced and visible; exceeding the cap stops the drain with a clear reason
- [ ] Mid-drain session expiry re-raises auth for that publisher only
- [ ] `needs_manual` is reported as a short actionable list with a reason per item

## References

- `crates/scitadel-adapters/src/download.rs` (`AccessStatus::Paywall`)
- Consumer-side prior art, working today, worth stealing from:
  `raid/tools/fetch/fulltext.py` (routes A–E incl. publisher TDM with
  `tdm_key_missing` fallback), `raid/tools/fetch/browser_cdp.py`
  (attach-to-user's-Chrome over CDP, one tab, ≥20 s/paper, never crawls),
  `raid/scripts/setup-fulltext-keys.sh` (hidden-input credential setup,
  fingerprint-only reporting, live `verify`, per-corpus coverage report).
- Related: #210 (DOI→metadata lookup).

---
*Filed from the raid project, which is the first consumer that needs tiers 2–4.
Happy to contribute the implementation — the consumer-side routes above are
already working and would mostly be ported, not invented.*

---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 11:14 PM_

Companion issue: #234 covers everything around this access ladder: the per-work artefact manifest (full text + SI + structured tables), Europe PMC/JATS and OSTI routes, SI harvesting, per-artefact provenance/licence, batch `acquire`/`coverage`, and the grouped `action_list` (meant to merge with `auth_pending()` here). Both should share one route ladder and one artefact store.

