---
name: code_execute
description: Works through an implementation plan in batches with human checkpoints.
disable-model-invocation: true
---

# Execute Plan

Work through an implementation plan in batches with human checkpoints.
Progress is tracked in the **GitHub issue comment** that contains the plan.

## Precondition: Issue Branch Required

Before doing anything else, verify you are on an issue branch:

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-*` (e.g. `feature/63-worktree-support`).
3. Extract the `<issue_number>` from the branch name.
4. If the branch does not match, **stop** and tell the user:
   - They need to be on an issue branch.
   - Offer to run [issue_claim](../issue_claim/SKILL.md) to create one.

## Workflow Steps

### 1. Load the plan from GitHub

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Fetch issue comments:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/comments \
     --jq '.[] | select(.body | contains("## Implementation Plan")) | {id, body}'
   ```

3. If multiple comments match, use the **most recent** one.
4. If no comment contains `## Implementation Plan`, **stop** and tell the user to run [design_plan](../design_plan/SKILL.md) first.
5. Parse the task list from the comment body. `- [ ]` = pending, `- [x]` = done.
6. Save the **comment ID** — you'll need it to edit the comment later.

### 2. Execute in batches

- Work through **unchecked** tasks sequentially, 2-3 tasks per batch.
- For each task:
  1. Announce which task you're starting.
  2. Implement the change (following [coding-principles](../../rules/coding-principles.mdc) and [tdd.mdc](../../rules/tdd.mdc)). Commit each phase via [git_commit](../git_commit/SKILL.md).
  3. Run the task's verification step.
  4. Report result (pass/fail with evidence).

### 3. Update progress after each batch

After completing a batch, check off finished tasks by editing the plan comment:

1. Re-fetch the comment to get the latest body (avoids overwriting concurrent edits):

   ```bash
   gh api repos/{owner}/{repo}/issues/comments/{comment_id} --jq '.body'
   ```

2. Replace `- [ ] Task description` with `- [x] Task description` for completed tasks.
3. Update the comment:

   ```bash
   gh api repos/{owner}/{repo}/issues/comments/{comment_id} \
     -X PATCH -f body="<updated_body>"
   ```

### 4. Checkpoint after each batch

- After updating, stop and show the user:
  - Tasks completed in this batch
  - Verification results
  - Tasks remaining (still unchecked)
- Wait for the user to say "continue" before starting the next batch.

### 5. Handle failures

- If a verification step fails, stop the batch.
- Diagnose using [code_debug](../code_debug/SKILL.md) principles if needed.
- Fix the issue before continuing to the next task.
- Do not skip failing tasks.

### 6. Wrap up

- After all tasks are done, run the full test suite: `just test`
- Report final status.
- Suggest committing and proceeding to [pr_create](../pr_create/SKILL.md).

## Important Notes

- **Do not run** without being on an issue branch. No exceptions.
- Never skip a checkpoint. The user must approve each batch.
- Each task should result in a working, testable state.
- If the plan needs adjustment mid-execution, edit the plan comment on the issue and get user approval before continuing.
- The plan comment is the **single source of truth**. No local plan files.
