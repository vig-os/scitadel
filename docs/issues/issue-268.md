---
type: issue
state: open
created: 2026-10-02T15:54:53Z
updated: 2026-10-02T15:54:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/268
comments: 0
labels: feature, effort:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-03T07:15:42.024Z
---

# [Issue 268]: [feat(adapters): E4 — live API contract tests, which are still unimplemented](https://github.com/vig-os/scitadel/issues/268)

## Problem

DES-002 lists as deliverable **E4**: "Contract tests against live APIs — gated behind `#[cfg(feature = "contract-tests")]` — catch API drift early without blocking CI." It has never been implemented.

The scaffolding existed but was empty in both halves: `crates/scitadel-adapters/Cargo.toml` declared `contract-tests = []` with nothing gated behind it, and the CI job that would have run it was unreachable (fixed in #267). `cargo test --features contract-tests` runs the same 552 tests as `cargo test --workspace`.

So there is currently **no detection for upstream API drift** — the failure mode this was meant to catch. When PubMed, OpenAlex, arXiv, INSPIRE-HEP, EPO, PatentsView or Lens change a response shape, the adapters break silently until a user notices a search returning nothing.

## Why this is not just "add a feature flag"

A feature flag that gates nothing is what made this invisible for the life of the project. The deliverable has to arrive as tests *plus* the mechanism, or it is the same dead config again.

## Proposal

1. **Gated tests that actually hit the network.** Each adapter gets a small number of `#[cfg(feature = "contract-tests")]` tests that make one real, cheap request and assert the response *parses into the adapter's own types* — not that specific field values are correct (those drift for benign reasons and produce a red build nobody trusts). The contract being protected is "the shape still deserialises".
2. **Recorded fixtures, per DES-002's original intent.** Where a parse cannot be asserted against a live call alone, record one sanitised response per adapter and assert the recorded shape still parses. Keeps most of the value offline.
3. **A trigger that can fire.** Per DES-002: `schedule:` plus `workflow_dispatch`, so a red contract test can be reproduced on demand rather than waiting for the cron. Needs credentials where a source requires them, so the job should skip-with-notice rather than fail when secrets are absent.
4. **A test that the gate is real.** Assert the feature-gated suite runs a *different, larger* set of tests than the default suite. This is the check whose absence let E4 rot: a cheap count assertion would have caught it the day the feature was added empty.

## Acceptance

- [ ] `cargo test --features contract-tests` runs strictly more tests than the default suite, and that difference is asserted by a test so it cannot silently regress to zero.
- [ ] A CI job runs the feature-gated suite on a schedule and on demand.
- [ ] Each of the seven adapters has at least one drift-detecting check.

## Related

- #239 / #267 (removed the unreachable job and the empty feature), DES-002 E4.
