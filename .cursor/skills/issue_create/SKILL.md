---
name: issue_create
description: Creates a new GitHub issue using the appropriate issue template.
disable-model-invocation: true
---

# Create a GitHub Issue

Create a new GitHub issue using the appropriate issue template.

## Workflow Steps

1. **Gather context from open issues**
   - Run `just gh-issues` to get an overview of all open issues, milestones, parent/child relationships, and open PRs.
   - Read `.github/ISSUE_TEMPLATE/` templates and `.github/label-taxonomy.toml` for correct labels.
   - Use this context to:
     - Avoid creating duplicates of existing issues
     - Suggest whether the new issue should be a sub-issue of an existing parent
     - Suggest an appropriate milestone based on the current backlog

2. **Determine issue type from context**
   - Infer which template to use based on the user's description:
     - Bug → `bug` (label: `bug`)
     - Feature/enhancement → `feature` (label: `feature`)
     - Refactoring → `refactor` (label: `refactor`)
     - Documentation → `docs` (label: `docs`)
     - CI/Build change, general task, maintenance → `chore` (label: `chore`)
   - Canonical labels are defined in `.github/label-taxonomy.toml` (single source of truth).
   - Ask the user if ambiguous.

3. **Populate fields from conversation context**
   - Draft a title following the template's prefix (e.g. `[FEATURE] ...`, `[BUG] ...`).
   - Draft the body with all required fields from the chosen template.
   - Include a Changelog Category value based on the issue type.
   - For testable issue types (`feature`, `bug`, `refactor`), include a TDD acceptance criterion:
     `- [ ] TDD compliance (see .cursor/rules/tdd.mdc)`

4. **Show draft and ask for confirmation**
   - Present the title, labels, and body to the user.
   - Wait for approval or edits before proceeding.

5. **Create the issue**

   ```bash
   gh issue create --title "<title>" --label "<label>" --body "<body>"
   ```

6. **Report the issue URL**
   - Show the user the created issue URL and number.

## Important Notes

- Canonical labels are defined in `.github/label-taxonomy.toml`. When unsure, check `gh label list` or read the taxonomy file.
- Do not create the issue until the user has approved the draft.
- If the user wants to start working on it immediately, follow up with the [issue_claim](../issue_claim/SKILL.md) workflow.
