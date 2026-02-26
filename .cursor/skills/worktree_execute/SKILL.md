---
name: worktree_execute
description: Autonomous TDD implementation — commits as it goes, no user checkpoints.
disable-model-invocation: true
---

# Autonomous Execute

Work through an implementation plan **without user checkpoints**. This is the worktree variant of [code_execute](../code_execute/SKILL.md). Progress is tracked in the GitHub issue comment.

**Rule: no blocking for feedback. Commit after each task. Follow TDD.**

## Precondition: Issue Branch Required

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-<summary>` (e.g. `feature/79-declarative-sync-manifest`). See [branch-naming.mdc](../../rules/branch-naming.mdc) for the full convention.
3. Extract the `<issue_number>` from the branch name.
4. If the branch does not match, **stop** and log the error.

## Workflow Steps

### 1. Load the plan from GitHub

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Fetch the most recent `## Implementation Plan` comment:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/comments \
     --jq '.[] | select(.body | contains("## Implementation Plan")) | {id, body}' | tail -1
   ```

3. If no plan exists, invoke [worktree_plan](../worktree_plan/SKILL.md) first.
4. Parse the task list: `- [ ]` = pending, `- [x]` = done.
5. Save the **comment ID** for progress updates.

### 2. Execute tasks sequentially

For each unchecked task:

1. Read the task description, files, and verification command.
2. Implement the change following [coding-principles](../../rules/coding-principles.mdc) and [tdd.mdc](../../rules/tdd.mdc):
   - **RED**: Write failing test, run it, confirm failure, commit via [git_commit](../git_commit/SKILL.md) (`test: ...`).
   - **GREEN**: Write minimal code to pass, run test, confirm pass, commit via [git_commit](../git_commit/SKILL.md) (`feat: ...` or `fix: ...`).
   - **REFACTOR**: Clean up if needed, run tests, commit via [git_commit](../git_commit/SKILL.md) (`refactor: ...`).
3. Run the task's verification step.
4. If verification fails, debug and fix before moving to the next task.

### 3. Update progress after each task

After completing a task, check it off in the plan comment:

1. Re-fetch the comment to get the latest body:

   ```bash
   gh api repos/{owner}/{repo}/issues/comments/{comment_id} --jq '.body'
   ```

2. Replace `- [ ] Task description` with `- [x] Task description`.
3. Update the comment:

   ```bash
   gh api repos/{owner}/{repo}/issues/comments/{comment_id} \
     -X PATCH -f body="<updated_body>"
   ```

### 4. Handle failures

- If a verification step fails, diagnose and fix immediately.
- Do not skip failing tasks.
- If genuinely stuck after 2-3 attempts, use [worktree_ask](../worktree_ask/SKILL.md) to post a question on the issue.

### 5. Proceed to verification

After all tasks are done, invoke [worktree_verify](../worktree_verify/SKILL.md) for full-suite verification.

## Delegation

The following steps SHOULD be delegated to reduce token consumption:

- **Step 1** (precondition check, load plan): Spawn a Task subagent with `model: "fast"` that validates the branch name, fetches the `## Implementation Plan` comment via `gh api`, parses the task list, and returns: issue number, comment ID, list of pending/completed tasks.
- **Step 3** (update progress): Spawn a Task subagent with `model: "fast"` that re-fetches the comment, performs the checkbox replacement, and updates the comment via `gh api`. Returns: success confirmation.
- **Step 5** (invoke next skill): Can remain in main agent (simple skill invocation).

Steps 2 and 4 (execute tasks, handle failures) should remain in the main agent as they require code generation, TDD discipline, and debugging.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Important Notes

- Never block waiting for user input. Execute tasks continuously.
- Each task should leave the codebase in a working, testable state.
- Skip TDD for non-testable changes (config, templates, docs) — note why in the commit.
- The plan comment is the single source of truth for progress.
- **NEVER add 'Co-authored-by: Cursor <cursoragent@cursor.com>'** to commit messages.
