---
name: code_review
description: Spawns a fresh-context readonly subagent to review changes before PR.
disable-model-invocation: true
---

# Self-Review

Structured self-review of changes before submitting a PR, executed by a **readonly subagent** for unbiased, fresh-context analysis.

## Why a Subagent?

The agent that wrote the code is biased toward its own output. A subagent starts with zero context — it only sees the diff, the issue, and the project standards. This catches blind spots the implementation agent misses.

## Workflow Steps

### 1. Collect inputs for the subagent

Before spawning the subagent, gather the raw data it needs:

```bash
# Determine base branch
BASE=$(gh pr view --json baseRefName --jq '.baseRefName' 2>/dev/null)
: "${BASE:=$(gh repo view --json defaultBranchRef --jq '.defaultBranchRef.name')}"

# Get the diff stat and commit log
git diff "$BASE"...HEAD --stat
git log "$BASE"..HEAD --oneline

# Get the linked issue number from the branch name
BRANCH=$(git branch --show-current)
ISSUE=$(echo "$BRANCH" | grep -oE '[0-9]+' | head -1)

# Fetch issue details
gh issue view "$ISSUE" --json title,body,labels
```

### 2. Spawn readonly review subagent

Use the `Task` tool to launch a **readonly** subagent (`readonly: true`). Pass it a prompt containing:

1. The diff stat and commit log from step 1.
2. The issue title, body, and acceptance criteria.
3. The review instructions below (copy them verbatim into the prompt).

The subagent must **not** modify any files. It only reads and reports.

#### Review instructions to include in the subagent prompt

```
You are a code reviewer. You have fresh context — you did not write this code.
Review the changes on this branch against the linked issue and project standards.

INPUTS (provided below):
- Diff stat and commit log
- Issue title, body, and acceptance criteria
- Project root is the current working directory

STEPS:

1. Read the full diff: git diff <BASE>...HEAD
2. Read the issue acceptance criteria provided above.
3. For each acceptance criterion, verify it is addressed in the diff.
   Flag any criterion NOT covered.
   Flag any change NOT traceable to a requirement (scope creep).
4. Check project standards:
   - Changelog: is CHANGELOG.md updated under ## Unreleased? Does the entry match?
   - Commit messages: do all commits follow the format in .cursor/rules/commit-messages.mdc?
   - Tests: are there tests for new/changed behavior?
   - Docs: are documentation changes needed?
5. Produce your report in EXACTLY this structure:

## Review: <branch> → <base>

### Acceptance Criteria
- [x] Criterion 1 — covered by <file/commit>
- [ ] Criterion 2 — NOT addressed

### Issues
- **Critical**: <blocks merge> (if any)
- **Important**: <should fix before merge> (if any)
- **Minor**: <nice to have> (if any)

### Assessment
Ready to submit / Needs fixes before PR

Return ONLY the review report. No preamble.
```

### 3. Act on the review report

When the subagent returns:

- If **Critical** or **Important** issues found → fix them, then re-run from step 1.
- If only **Minor** issues → note them and proceed to [pr_create](../pr_create/SKILL.md).

## Delegation

The subagent spawned in step 2 SHOULD use `model: "fast"` since code review is a structured analysis task with clear inputs (diff, issue, standards) and a fixed output format.

Update step 2's Task tool invocation to include:

```markdown
Task tool parameters:
- `readonly: true` (already specified)
- `model: "fast"` (add this — review fits the "standard" tier pattern)
- `description: "Code review: branch vs base"`
```

This reduces token consumption on the primary model while maintaining review quality, as the review checklist is well-defined and the subagent has all necessary context.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Important Notes

- Run this before every PR submission. The [pr_create](../pr_create/SKILL.md) workflow should reference this as a prerequisite.
- Do not skip the acceptance criteria check — it catches the most common agent failure (incomplete work).
- The subagent runs readonly — it cannot modify files. All fixes are made by the calling agent.
