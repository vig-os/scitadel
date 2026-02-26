---
name: design_plan
description: Breaks an approved design or issue into bite-sized implementation tasks.
disable-model-invocation: true
---

# Write Implementation Plan

Break an approved design or issue into bite-sized implementation tasks.

## Precondition: Issue Branch Required

Before doing anything else, verify you are on an issue branch:

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-*` (e.g. `feature/63-worktree-support`).
3. Extract the `<issue_number>` from the branch name.
4. If the branch does not match, **stop** and tell the user:
   - They need to be on an issue branch.
   - Offer to run [issue_claim](../issue_claim/SKILL.md) to create one.

## Workflow Steps

### 1. Read the issue

- Run: `gh issue view <issue_number> --json title,labels,body`
- Read acceptance criteria, implementation notes, and constraints from the issue body.
- Check issue comments for an existing design (look for `## Design` heading) for additional context.

### 2. Break into tasks

- Each task should be completable in 2-5 minutes.
- Each task must specify:
  - **What**: one sentence describing the change
  - **Files**: exact file paths to create or modify
  - **Verification**: how to confirm the task is done (e.g. `just test`, specific test passes)
- Order tasks by dependency — earlier tasks should not depend on later ones.

### 3. Identify test tasks

- For each functional task, include a corresponding test task (or note that the test is part of the same task).
- Follow TDD: test tasks come before or alongside implementation tasks, not after.

### 4. Present plan for approval

- Show the full task list to the user.
- Ask for confirmation or adjustments before proceeding.

### 5. Publish the plan as a GitHub issue comment

After user approval, post the full detailed plan as a **comment on the issue**. This is the single source of truth.

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Post the plan comment:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/comments \
     -f body="<plan_content>"
   ```

3. The comment must start with `##` (H2) so other skills can detect that the planning phase is complete.
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

## Important Notes

- **Do not run** without being on an issue branch. No exceptions.
- Do not start implementation until the user approves the plan.
- If a task is too large to describe in one sentence, split it.
- Reference specific `just` recipes for verification where applicable.
- The issue comment is the **single source of truth** for the plan. No local plan files.
- The plan comment is the input for [code_execute](../code_execute/SKILL.md).
