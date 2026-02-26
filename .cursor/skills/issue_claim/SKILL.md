---
name: issue_claim
description: Sets up the local environment to begin working on a GitHub issue, and ensures the issue is assigned.
disable-model-invocation: true
---

# Claim and Start Work on an Issue

Set up the local environment to begin working on a GitHub issue, and ensure the issue is assigned to you.

## Workflow Steps

1. **Identify the issue**
   - The user will reference an issue number (e.g. "start issue 63", "work on #63", or a `.github_data/issues/issue-63.md` file).
   - Run `gh issue view <number> --json title,labels,body,assignees` to get context.

2. **Check assignment**
   - Inspect the `assignees` list from step 1.
   - **Nobody assigned:** offer to assign the current user (`gh issue edit <number> --add-assignee @me`). Proceed after the user confirms or declines.
   - **Current user already assigned:** note it and continue — no action needed.
   - **Someone else assigned:** warn the user that the issue is already assigned to that person. Ask whether to proceed (and optionally co-assign with `--add-assignee @me`) or stop.

3. **Check for existing linked branch**
   - Run: `gh issue develop --list <issue_number>`
   - If a branch already exists, offer to check it out: `git fetch origin && git checkout <branch>`.
   - Do not create a second linked branch.

4. **Stash dirty working tree if needed**
   - Run `git status --short`. If there are uncommitted changes, run `git stash push -u -m "before-issue-<number>"` and tell the user.

5. **Determine base branch**
   - Check if the issue has a parent: `gh api repos/{owner}/{repo}/issues/{issue_number}/parent --jq '.number'`
   - If a parent exists, resolve its linked branch: `gh issue develop --list <parent_number>`. Use the parent's branch as `<base_branch>`. If the parent has no linked branch, fall back to `dev`.
   - If no parent exists, use `dev` as `<base_branch>`.

6. **Follow the branch naming rule**
   - Apply the workflow in [branch-naming.mdc](../../rules/branch-naming.mdc): infer type, derive short summary, propose branch name, wait for user confirmation.
   - Pass the detected `<base_branch>` to the branch creation step.

7. **Create and link the branch**
   - After user confirms: `gh issue develop <issue_number> --base <base_branch> --name <branch_name> --checkout`
   - Then: `git pull origin <branch_name>`

8. **Restore stash if applicable**
   - If you stashed in step 4: `git stash pop`

## Important Notes

- Always ask the user to confirm the branch name before creating it.
- If `gh issue develop` fails because the branch already exists on remote, run `git fetch origin && git checkout <branch_name>` instead.
- Read the issue body after checkout so you have context for the work ahead.
- Determine the current GitHub user with `gh api user --jq '.login'` when comparing against assignees.
