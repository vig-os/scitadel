---
name: worktree_pr
description: Autonomous PR creation from a worktree branch.
disable-model-invocation: true
---

# Autonomous PR

Create a pull request **without user interaction**. This is the worktree variant of [pr_create](../pr_create/SKILL.md).

**Rule: no blocking for feedback. Auto-generate PR text from commits and issue.**

## Precondition: Issue Branch Required

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-<summary>` (e.g. `feature/79-declarative-sync-manifest`). See [branch-naming.mdc](../../rules/branch-naming.mdc) for the full convention.
3. Extract the `<issue_number>` from the branch name.

## Workflow Steps

### 1. Determine base branch

Detect whether this issue is a sub-issue and resolve the correct merge target:

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Check for a parent issue:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/parent --jq '.number'
   ```

3. If a parent exists, resolve its linked branch:

   ```bash
   gh issue develop --list <parent_number>
   ```

   - Use the parent's branch as `<base_branch>`.
   - If the parent has no linked branch, fall back to `dev`.

4. If no parent exists, use `dev` as `<base_branch>`.

### 2. Ensure clean state

```bash
git status
git fetch origin
```

- If there are uncommitted changes, commit them first.
- **Merge the base branch** before pushing:

```bash
  git merge origin/<base_branch>
  ```

**Conflict handling:** If merge conflicts occur, list the conflicting files and invoke [worktree_ask](../worktree_ask/SKILL.md) to post a question on the issue asking for help resolving the conflict. Do not push until conflicts are resolved.
- Push the branch: `git push -u origin HEAD`

### 3. Gather context

```bash
git log <base_branch>..HEAD --oneline
git diff <base_branch>...HEAD --stat
gh issue view <issue_number> --json title,body
```

- Read the issue title and acceptance criteria.
- Summarize what the commits accomplish.

### 4. Ensure CHANGELOG is updated

- Check `CHANGELOG.md` for an entry under `## Unreleased` that covers the changes.
- If missing, add the appropriate entry and commit.

### 5. Generate PR text

1. **Read the template**: `cat .github/pull_request_template.md`
2. **Use it as the literal skeleton** — keep every heading, every checkbox line, every sub-heading. Strip only the HTML comments (`<!-- ... -->`).
3. **Section-by-section mapping**:
   - **Description**: Summarize what the PR does from the issue body and commit messages.
   - **Type of Change**: Check the single box matching the branch type / commit types. Check `Breaking change` modifier only if commits contain `!`.
   - **Changes Made**: List changed files with bullet sub-details (from `git diff --stat` and `git log`).
   - **Changelog Entry**: Paste the exact `## Unreleased` diff from CHANGELOG.md. If no changelog update, write "No changelog needed" and explain.
   - **Testing**: Check `Tests pass locally` if tests were run. Check `Manual testing performed` only if actually done. Fill `Manual Testing Details` or write "N/A".
   - **Checklist**: Check only items that are genuinely true. Leave unchecked items unchecked — do not remove them.
   - **Additional Notes**: Add design links, context, or write "N/A".
   - **Refs**: `Refs: #<issue_number>`
4. **Explicit prohibitions**: Do not invent new sections. Do not rename headings. Do not omit sections. Do not remove unchecked boxes.
5. Write the body to `.github/pr-draft-<issue_number>.md`.

### 6. Create PR

```bash
# Append reviewer if PR_REVIEWER is set in environment
REVIEWER_ARG=""
if [ -n "${PR_REVIEWER:-}" ]; then
  REVIEWER_ARG="--reviewer $PR_REVIEWER"
fi

gh pr create --base <base_branch> --title "<type>: <description> (#<issue_number>)" \
  --body-file .github/pr-draft-<issue_number>.md \
  --assignee @me $REVIEWER_ARG
```

If the `WORKTREE_REVIEWER` environment variable is set (populated by `just worktree-start`), add the reviewer:

```bash
gh pr create --base <base_branch> --title "<type>: <description> (#<issue_number>)" \
  --body-file .github/pr-draft-<issue_number>.md \
  --assignee @me \
  --reviewer "$WORKTREE_REVIEWER"
```

The reviewer is the person who launched the worktree (their gh user login), not the agent.

### 7. Clean up

- Delete the draft file: `rm .github/pr-draft-<issue_number>.md`
- Report the PR URL.

## Delegation

The following steps SHOULD be delegated to reduce token consumption:

- **Steps 1-2** (precondition check, determine base branch, ensure clean state): Spawn a Task subagent with `model: "fast"` that validates the branch name, checks for a parent issue via `gh api`, resolves the base branch, runs `git status`/`git fetch`, merges `origin/<base_branch>`, and pushes. Returns: issue number, base branch name, clean state confirmation. On merge conflict, the subagent must invoke worktree_ask and return without pushing.
- **Step 3** (gather context): Spawn a Task subagent with `model: "fast"` that executes `git log`, `git diff`, `gh issue view` and returns the raw outputs. Returns: commit log, diff stat, issue title/body.
- **Steps 6-7** (create PR, clean up): Spawn a Task subagent with `model: "fast"` that takes the PR title and body file path, executes `gh pr create`, deletes the draft file, and returns the PR URL.

Steps 4-5 (ensure CHANGELOG updated, generate PR text) should remain in the main agent as they require understanding changes and writing structured content.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Important Notes

- Never block for user review of the PR text. Generate the best text from available context.
- Base branch is auto-detected: parent issue's branch for sub-issues, `dev` otherwise.
- The PR title should follow commit message conventions: `type(scope): description (#issue)`.
- **NEVER add 'Co-authored-by: Cursor <cursoragent@cursor.com>'** to commit messages.
