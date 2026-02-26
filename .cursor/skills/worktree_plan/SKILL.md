---
name: worktree_plan
description: Autonomous planning — reads issue and design, posts implementation plan, never blocks.
disable-model-invocation: true
---

# Autonomous Plan

Break an approved design into implementation tasks **without user interaction**. This is the worktree variant of [design_plan](../design_plan/SKILL.md).

**Rule: no implementation until a plan is posted. No blocking for feedback.**

## Precondition: Issue Branch Required

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-<summary>` (e.g. `feature/79-declarative-sync-manifest`). See [branch-naming.mdc](../../rules/branch-naming.mdc) for the full convention.
3. Extract the `<issue_number>` from the branch name.
4. If the branch does not match, **stop** and log the error.

## Workflow Steps

### 1. Read the full issue and design

```bash
gh issue view <issue_number> --json title,body,labels,comments
```

- Parse the **body** for acceptance criteria and constraints.
- Find the `## Design` comment for the approved architecture.
- If an `## Implementation Plan` comment already exists, **skip** — the planning phase is done.
- If no `## Design` comment exists, invoke [worktree_brainstorm](../worktree_brainstorm/SKILL.md) first.

### 2. Break into tasks

- Each task should be completable in 2-5 minutes.
- Each task must specify:
  - **What**: one sentence describing the change.
  - **Files**: exact file paths to create or modify.
  - **Verification**: how to confirm the task is done (e.g. `just test`, specific test passes).
- Order tasks by dependency — earlier tasks must not depend on later ones.
- Follow TDD: test tasks come before or alongside implementation tasks.

### 3. Publish plan as a GitHub issue comment

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Post the plan comment:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/comments \
     -f body="<plan_content>"
   ```

3. The comment **must** start with `## Implementation Plan` (H2) — this is how other skills detect that the planning phase is complete.
4. Use this format:

   ```markdown
   ## Implementation Plan

   Issue: #<issue_number>
   Branch: <branch_name>

   ### Tasks

   - [ ] Task 1: description — `files` — verify: `command`
   - [ ] Task 2: description — `files` — verify: `command`
   ...
   ```

### 4. Proceed to execution

- Invoke [worktree_execute](../worktree_execute/SKILL.md) to start implementing.

## Delegation

The following steps SHOULD be delegated to reduce token consumption:

- **Steps 1, 4** (precondition check, read issue/design): Spawn a Task subagent with `model: "fast"` that validates the branch name, executes `gh issue view`, checks for existing `## Design` and `## Implementation Plan` comments. Returns: issue number, parsed body/design, plan-exists flag.
- **Step 3** (publish plan): Spawn a Task subagent with `model: "fast"` that takes the formatted plan content and posts it via `gh api`. Returns: comment URL.
- **Step 4** (invoke next skill): Can remain in main agent (simple skill invocation).

Step 2 (break into tasks) should remain in the main agent as it requires task decomposition and dependency analysis.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Important Notes

- Never block waiting for user input. Make reasonable task breakdowns and move on.
- If a task is too large to describe in one sentence, split it.
- The plan comment is the single source of truth — no local plan files.
