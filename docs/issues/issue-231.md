---
type: issue
state: closed
created: 2026-09-28T17:39:26Z
updated: 2026-09-28T23:17:26Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/scitadel/issues/231
comments: 1
labels: docs, effort:small, priority:low, area:docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:24.020Z
---

# [Issue 231]: [docs(release): drop the pending-deletion note for the retired release-please credentials](https://github.com/vig-os/scitadel/issues/231)

### Description

`docs/RELEASING.md` (§ *One-time setup*, lines 74-75 on `dev`) still describes
the retired release-please credentials as pending deletion:

> The retired release-please credentials (`RELEASE_PLEASE_TOKEN`,
> `RELEASE_BOT_APP_ID`, `RELEASE_BOT_PRIVATE_KEY`) can be deleted.

They are now gone. vig-os/org-config#295 dropped their declarations from the org
config (vig-os/org-config#297, merged `3cfdc30`), and the live values were
deleted from this repo on 2026-09-28. `gh secret list` shows only
`CARGO_REGISTRY_TOKEN` and `gh variable list` is empty.

### Documentation Type

Fix incorrect or outdated content

### Target Files

- `docs/RELEASING.md`: remove the paragraph, or restate it in the past tense
  (for example, "were deleted on 2026-09-28, vig-os/org-config#295") if the
  history is worth keeping next to the setup table.

### Related Code Changes

- vig-os/org-config#295 / vig-os/org-config#297: config-side removal
- #207: release-please retirement
- #209: closed, its numeric-App-ID migration is moot

### Acceptance Criteria

- [ ] `docs/RELEASING.md` no longer presents the three credentials as live or pending deletion
- [ ] `CARGO_REGISTRY_TOKEN` row unchanged (still the trusted-publishing fallback)

---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 11:17 PM_

Fixed in #233: the credentials note and the stale first-release runbook are now a short "Migration history" section in `docs/RELEASING.md`.

