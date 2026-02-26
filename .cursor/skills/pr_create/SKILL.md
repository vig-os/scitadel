---
name: pr_create
description: Prepares and submits a pull request for feature or bugfix work.
disable-model-invocation: true
---

# Submit Pull Request

Prepare and submit a pull request for **feature or bugfix work**.

> **Note:** This workflow is for regular development PRs (feature/bugfix branches to `dev`).
> For **release PRs**, see [../docs/RELEASE_CYCLE.md](../docs/RELEASE_CYCLE.md) — releases are automated via `prepare-release.sh`.

## Workflow Steps

### 1. Ensure Git is up to date

- Run `git status` and `git fetch origin`. If the current branch has a remote tracking branch, run `git pull --rebase origin <current-branch>` (or `git pull` if the user prefers merge) so the branch is up to date with the remote.
- If there are uncommitted changes, list them and ask the user to commit or stash before submitting the PR. Do not prepare the PR until the working tree is clean (or the user explicitly says to proceed with uncommitted changes).
- **Merge the base branch:** Once the base branch is confirmed (step 2), run `git merge origin/<base_branch>` to integrate the latest base before creating the PR. **Conflict handling:** If merge conflicts occur, list the conflicting files and ask the user to resolve them manually before proceeding.

### 2. Verify target branch

- Confirm the **base (target) branch** for the PR (e.g. `dev`, `feature/37-automate-standardize-repository-setup`). If the user did not specify it, infer from context (e.g. "into 37" → branch for issue 37) or ask. Use `gh issue develop --list <issue>` if needed to resolve a branch name from an issue number.

### 3. Ensure CHANGELOG has been updated

- Compare the list of commits (and/or files changed) on the current branch vs the base branch to the **Unreleased** section of `CHANGELOG.md`.
- Every user-facing or notable change in the PR must be documented under Unreleased (Added, Changed, Fixed, etc.). If something is missing, add the corresponding bullet(s) to `CHANGELOG.md` and tell the user what you added, or prompt the user to update the CHANGELOG before submitting.

### 4. Prepare PR text following template

1. **Read the template**: `cat .github/pull_request_template.md`
2. **Use it as the literal skeleton** — keep every heading, every checkbox line, every sub-heading. Strip only the HTML comments (`<!-- ... -->`).
3. **Section-by-section mapping**:
   - **Description**: Summarize what the PR does from the issue body and commit messages.
   - **Type of Change**: Check the single box matching the branch type / commit types. Check `Breaking change` modifier only if commits contain `!`.
   - **Changes Made**: List changed files with bullet sub-details (from `git diff --stat base...HEAD` and `git log base..HEAD`).
   - **Changelog Entry**: Paste the exact `## Unreleased` diff from CHANGELOG.md. If no changelog update, write "No changelog needed" and explain.
   - **Testing**: Check `Tests pass locally` if tests were run. Check `Manual testing performed` only if actually done. Fill `Manual Testing Details` or write "N/A".
   - **Checklist**: Check only items that are genuinely true. Leave unchecked items unchecked — do not remove them.
   - **Additional Notes**: Add design links, context, or write "N/A".
   - **Refs**: `Refs: #<issue_number>`
4. **Explicit prohibitions**: Do not invent new sections. Do not rename headings. Do not omit sections. Do not remove unchecked boxes.
5. Write the body to a file (e.g. `.github/pr-draft-<issue>-into-<base>.md` or similar) so the user can edit it if needed.

### 5. Ask user to review and choose assignee and reviewers

- Show the user the **title** you will use (e.g. `feat: short description`) and the **PR body** (full markdown). Do **not** include the issue number in the title — GitHub automatically appends `(#PR)` to the merge commit subject, and the issue is traceable via `Refs:` in the body.
- Ask the user to confirm or edit the text.
- Ask the user to specify **assignee** and **reviewers** (e.g. "assign to me, no reviewers" or "assign @c-vigo, reviewers @foo"). Do not run `gh pr create` until the user approves and provides assignee/reviewers.

### 6. Submit PR

- Run:

  ```bash
  gh pr create --base <target-branch> --title "<title>" --body-file <path-to-draft> [--assignee <login>] [--reviewer <login> ...]
  ```

- Use the approved title and body file. Add `--assignee` and `--reviewer` only as specified by the user.
- After the PR is created, tell the user the PR URL and that they can delete the draft body file if they want.

## Important Notes

- Default branch for "into 37" is `feature/37-automate-standardize-repository-setup` (or the result of `gh issue develop --list 37`). Confirm with the user when ambiguous.
- If CHANGELOG is missing entries, add them in the same style as existing Unreleased items; do not leave the PR without CHANGELOG updates for new changes.
- Never submit the PR (step 6) until the user has approved the text and provided assignee/reviewers preferences.
