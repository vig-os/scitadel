---
name: issue_triage
description: Triage open GitHub issues by analyzing them across priority, area, effort, SemVer impact, dependencies, and release readiness. Groups related issues into parent/sub-issue clusters, suggests milestone assignments, and applies approved changes via gh CLI. Use when the user asks to triage issues, groom the backlog, plan a milestone, or organize open issues.
---

# Issue Triage

Perform a full triage of all open issues in the current GitHub repo. Analyze
each issue across 7 dimensions, group related issues into parent/sub-issue
clusters, and suggest milestone assignments. All mutations require explicit
user approval.

## Phase 1: Collect

Gather all data needed for analysis. Run these commands and hold the results
in memory:

```bash
# Open issues (all fields needed for analysis)
gh issue list --state open --limit 200 \
  --json number,title,labels,milestone,assignees,body,createdAt,updatedAt

# Open PRs (readiness context + PR-to-issue mapping)
gh pr list --state open \
  --json number,title,headRefName,labels,milestone,body,reviewDecision

# Recently merged PRs (last 20 -- for issues that may be nearly done)
gh pr list --state merged --limit 20 \
  --json number,title,headRefName,mergedAt

# Milestones
gh api repos/{owner}/{repo}/milestones \
  --jq '.[] | {number,title,state,open_issues,closed_issues}'

# Labels
gh label list --json name,description,color

# Existing sub-issue relationships (for each issue, skip 404s)
# List sub-issues of an issue:
gh api repos/{owner}/{repo}/issues/{n}/sub_issues 2>/dev/null
# Get parent of an issue:
gh api repos/{owner}/{repo}/issues/{n}/parent 2>/dev/null
```

Also read `docs/issues/` for local issue markdown files if available.

Determine `{owner}/{repo}` with:

```bash
gh repo view --json nameWithOwner --jq '.nameWithOwner'
```

## Phase 2: Check label taxonomy

Read [`.github/label-taxonomy.toml`](../../../.github/label-taxonomy.toml) for the expected labels.

1. Compare the repo labels from Phase 1 against the taxonomy.
2. If any labels are missing, present them grouped by category (see example below).
3. Create approved labels with `gh label create`.

Example prompt for missing labels:

```
Missing labels:
  Priority: priority:blocking, priority:high, priority:medium, ...
  Area: area:ci, area:image, ...
Approve all / pick individually / skip?
```

Example label creation:

```bash
gh label create "priority:high" --color "d93f0b" \
  --description "Should be done in the current milestone"
```

## Phase 3: Analyze and build decision matrix

For each open issue, analyze the title, body, and existing labels to suggest
values across all 7 dimensions plus PR coverage:

| Dimension | Values | How to determine |
|-----------|--------|-----------------|
| **Type** | existing labels: `feature`, `bug`, `question`, `task`, etc. | Already on the issue |
| **Area** | `ci`, `image`, `workspace`, `workflow`, `docs`, `testing` | Keywords in title/body, files referenced |
| **Priority** | `blocking`, `high`, `medium`, `low`, `backlog` | Impact described in body, dependency chains, age |
| **Effort** | `small`, `medium`, `large` | Scope of change described, number of files/components |
| **SemVer** | `major`, `minor`, `patch` | Breaking vs additive vs fix |
| **Readiness** | `needs design`, `ready`, `in progress`, `done` | Linked PRs/branches, design docs in body |
| **Dependencies** | Issue numbers | Cross-references in bodies (#N, "depends on", "blocks") |
| **PR** | PR number or `—` | Linked open/merged PRs (see PR analysis below) |

### PR analysis

Use the open and recently merged PRs collected in Phase 1 to enrich the
issue analysis:

1. **Map PRs to issues.** For each PR, determine which issue(s) it addresses
   by matching:
   - Branch name pattern: `<type>/<issue_number>-...` (e.g. `feature/67-declarative-sync-manifest` → #67)
   - PR body keywords: `Refs: #N`, `Closes #N`, `Fixes #N`
   - PR title references: `#N` in the title

2. **Infer readiness from PR state:**
   | PR state | Issue readiness |
   |----------|----------------|
   | Open, review pending | `in progress` |
   | Open, changes requested | `in progress` (note: needs rework) |
   | Open, approved | `in progress` (ready to merge) |
   | Recently merged | `done` (or close to done — verify issue is closed) |
   | No PR exists | Keep existing readiness inference |

3. **Surface PR-based dependencies.** If issue A depends on issue B, and
   issue B has an open (unmerged) PR, then issue A is **blocked by PR #X**.
   Note this in the Deps column: `#B (PR #X)`.

4. **Identify issues without PRs.** For issues marked `ready` or higher
   priority that have no linked PR, flag them in the matrix as candidates
   for immediate work. Optionally suggest this in a "PR gaps" summary
   section after the matrix.

5. **Suggest PRs for review.** In the PR summary section, list open PRs
   with their review status so the user can identify PRs that need attention
   (e.g. approved but not merged, or waiting for review).

### Grouping into clusters

Identify clusters of related issues:

1. **Shared area** -- multiple issues with the same inferred area
2. **Cross-references** -- issues that reference each other (`#N`, "depends on", "blocks", "related to")
3. **Thematic similarity** -- issues about the same component or initiative

For each cluster, determine a parent:
- If an existing open issue has **epic-level scope** (broad title, multiple sub-tasks implied), suggest it as parent
- Otherwise, suggest **creating a new parent issue** with a title summarizing the cluster

Issues that don't belong to any cluster go in an **Ungrouped** section.

### Matrix format

Present as grouped tables, one per cluster:

```
## Triage Decision Matrix

### Cluster: "<theme>" (suggested parent: #N or NEW)
| # | Title | Type | Area | Priority | Effort | SemVer | Readiness | PR | Milestone | Deps |
|---|-------|------|------|----------|--------|--------|-----------|-----|-----------|------|
| P #N | Parent issue title... | ... | ... | ... | ... | ... | ... | #68 | ... | ... |
| └ #M | Sub-issue title... | ... | ... | ... | ... | ... | ... | — | ... | #X (PR #68) |

### Ungrouped
| # | Title | Type | Area | Priority | Effort | SemVer | Readiness | PR | Milestone | Deps |
|---|-------|------|------|----------|--------|--------|-----------|-----|-----------|------|
| #K | Standalone issue... | ... | ... | ... | ... | ... | ... | — | ... | ... |
```

Column key:
- **#**: `P` = parent, `P #N` = existing issue as parent, `└ #N` = sub-issue
- **PR**: linked open PR number, or `—` if none
- **Milestone**: suggest a SemVer milestone (`0.3`, `0.4`, etc.) or `backlog`
- **Deps**: issue numbers this issue depends on; append `(PR #X)` when the
  dependency is blocked by an unmerged PR

### PR summary section

After the cluster tables and before the milestone summary, add a **PR
Summary** section:

```
## PR Summary

### Open PRs
| PR | Issue | Branch | Review | Status |
|----|-------|--------|--------|--------|
| #68 | #67 | feature/67-... | pending | In progress |

### Issues without PRs (ready or higher priority)
| # | Title | Priority | Readiness | Suggested action |
|---|-------|----------|-----------|-----------------|
| #80 | Reconcile labels... | high | ready | Needs a PR |
```

This helps the user spot:
- PRs that need review attention (approved but unmerged, changes requested)
- High-priority issues with no active work
- Blocked dependency chains where merging a PR would unblock others

### Parent milestone convention

A parent issue represents a theme/initiative that may span multiple milestones.
**Convention:** parent issues should have **no milestone assigned** — they are
pure tracking issues that close when all sub-issues are done. Only sub-issues
(the actual work units) get milestone assignments. In the matrix, leave the
Milestone cell empty for parent rows.

### Write matrix to file

After building the decision matrix, write it to `.github_data/triage-matrix.md`.
Create the `.github_data/` directory if it does not exist. Write the full matrix
tables (grouped by cluster and ungrouped) to this file so the user can open and
edit it directly in their IDE. Do not rely on chat output alone — the file is
the canonical editable artifact.

## Phase 4: Present and get approval

1. Tell the user the matrix has been written to `.github_data/triage-matrix.md`.
2. Ask the user to open the file, review it, and edit any cells directly (priority,
   milestone, effort, cluster assignment, etc.).
3. When the user says they are done, re-read `.github_data/triage-matrix.md` and
   parse any changes before proceeding to Phase 5.
4. Use the parsed content (including user edits) as the source of truth for
   applying changes.

## Phase 5: Apply changes (batched)

Present each batch for approval before executing. Wait for confirmation
between batches.

### Batch 1: New parent issues

For each cluster where the parent is NEW:

```bash
gh issue create --title "<cluster theme>" --label "<labels>" \
  --body "<description referencing sub-issues>"
```

Report the created issue number.

### Batch 2: Sub-issue links

Link sub-issues to their parents using the GitHub sub-issues REST API:

```bash
# Get the node_id of the child issue
CHILD_NODE_ID=$(gh issue view {child_number} --json nodeId --jq '.nodeId')

# Add as sub-issue to parent
gh api repos/{owner}/{repo}/issues/{parent_number}/sub_issues \
  -f sub_issue_id="$CHILD_NODE_ID"
```

If the API returns 404, warn the user that sub-issues may not be enabled
for this repo and skip this batch.

### Batch 3: Label assignments

```bash
gh issue edit {n} --add-label "priority:high,area:ci,effort:small,semver:minor"
```

### Batch 4: Milestone assignments

Create new milestones if needed:

```bash
gh api repos/{owner}/{repo}/milestones -f title="0.4"
```

Assign milestones:

```bash
gh issue edit {n} --milestone "0.3"
```

### Batch 5: Summary

Print a summary of all changes made:
- New parent issues created (with numbers)
- Sub-issue links added
- Labels applied
- Milestones assigned
- Issues left unchanged (and why)

## Delegation

The following phases SHOULD be delegated to reduce token consumption:

- **Phase 1** (collect all data): Spawn a Task subagent with `model: "fast"` that executes all the gh/git commands listed in Phase 1 (issues, PRs, milestones, labels, sub-issue relationships). Returns: all raw JSON outputs combined into a structured response.
- **Phase 2** (check label taxonomy): Spawn a Task subagent with `model: "fast"` that reads `.github/label-taxonomy.toml`, compares against repo labels, and identifies missing labels grouped by category. Returns: missing label list formatted for user approval.
- **Phase 4** (present and wait): Can remain in main agent (user interaction, file writing).
- **Phase 5** (apply changes): Spawn a Task subagent with `model: "fast"` for each batch after approval is received. The subagent executes the gh commands and returns confirmation/error messages. Process batches sequentially, waiting for approval between each.

Phase 3 (analyze and build decision matrix) should remain in the main agent as it requires multi-dimensional analysis, clustering logic, and dependency inference.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Error Handling

- **404 on sub-issue endpoints**: Warn user that sub-issues may not be enabled. Skip sub-issue batches, continue with labels and milestones.
- **Label creation failure** (duplicate): Skip gracefully, the label already exists.
- **Milestone creation failure**: Report error, continue with other milestones.
- **Never retry destructive operations**: Report the failure and let the user decide.

## Important Notes

- **Never mutate without approval.** Every change is presented first and requires explicit confirmation.
- Milestones follow SemVer (e.g. `0.3`, `0.4`, `1.0`) matching the project release cycle.
- Existing sub-issue relationships discovered in Phase 1 should be preserved -- only add new links, never remove existing ones.
- If an issue already has a milestone, show it in the matrix but don't suggest changing it unless the user asks.
