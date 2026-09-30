---
type: issue
state: closed
created: 2026-09-29T11:17:31Z
updated: 2026-09-29T13:07:52Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/240
comments: 1
labels: chore
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:41:25.336Z
---

# [Issue 240]: [chore(deps): enable Renovate cargo manager and drop invalid uv manager](https://github.com/vig-os/scitadel/issues/240)

### Chore Type

Configuration change

### Description

`renovate.json` sets `enabledManagers` to `["github-actions", "pep621", "uv", "npm"]`, on both `main` and `dev`. Two problems:

1. **`uv` is not a Renovate manager.** It is not on the manager list (https://docs.renovatebot.com/modules/manager/); Renovate handles `uv.lock` through `pep621`. The validator rejects the config:
   `npx --yes --package renovate -- renovate-config-validator --strict renovate.json` fails with `The following managers configured in enabledManagers are not supported: "uv"`.
2. **`cargo` is not enabled**, so Renovate never proposes updates for the Rust workspace. The repo has no `pyproject.toml` or `package.json`, so `pep621` and `npm` match nothing either.

The org is moving vulnerability-fix PRs from Dependabot security updates to Renovate (vig-os/org-config#307). Renovate only opens a vulnerability PR for a dependency that an enabled manager extracts, so the Rust crates need `cargo`.

### Acceptance Criteria

- [ ] `enabledManagers` is `["github-actions", "cargo"]`
- [ ] `renovate-config-validator --strict renovate.json` passes

### Implementation Notes

Renovate reads its config from the default branch (`main`). A fix merged to `dev` takes effect only when the next release reaches `main`.

### Related Issues

Context: vig-os/org-config#307.

---

# [Comment #1]() by [c-vigo]()

_Posted on September 29, 2026 at 01:07 PM_

Done: #241 (dev) and #243 (main). Renovate reads its config from main, so the fix is live now.

