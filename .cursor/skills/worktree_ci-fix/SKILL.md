---
name: worktree_ci-fix
description: Autonomous CI fix — diagnoses failure, posts diagnosis, fixes, pushes, re-checks.
disable-model-invocation: true
---

# Autonomous CI Fix

Diagnose and fix a failing CI run **without user interaction**. This is the worktree variant of [ci_fix](../ci_fix/SKILL.md) — it posts a lightweight diagnosis comment for traceability, then fixes, pushes, and re-checks autonomously.

**Rule: no guessing. Fetch the log first. No blocking for feedback.**

## Precondition: Issue Branch Required

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-<summary>` (e.g. `feature/79-declarative-sync-manifest`). See [branch-naming.mdc](../../rules/branch-naming.mdc) for the full convention.
3. Extract the `<issue_number>` from the branch name.
4. If the branch does not match, **stop** and log the error.

## Workflow Steps

### 1. Investigate — get failure details

```bash
gh run list --branch $(git branch --show-current) --limit 5
gh run view <run-id> --log-failed
```

- Identify the failing workflow, job, and step.
- Read the full error output — line numbers, file paths, exit codes.

### 2. Analyze — root cause

- Open the relevant workflow in `.github/workflows/` or action in `.github/actions/`.
- Check recent changes: `git log --oneline -10` — what changed that could cause this?
- Compare with the last passing run — is this a new failure or pre-existing?
- Trace the data flow — what inputs does the failing step receive?

### 3. Post diagnosis comment

Before making any fix, post a `## CI Diagnosis` comment on the issue for traceability:

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Post:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/comments \
     -f body="<diagnosis_content>"
   ```

3. The comment **must** start with `## CI Diagnosis` (H2) and use this format:

   ```markdown
   ## CI Diagnosis

   **Failing workflow:** <workflow> / <job> / <step>
   **Error:** <key error line or message>
   **Root cause:** <one-sentence explanation>
   **Planned fix:** <what will be changed>
   ```

### 4. Fix

- Make the **smallest** change that addresses the root cause.
- Reproduce locally if possible (`just test`, `just lint`, `just precommit`).
- Commit following project conventions.
- Never use `--no-verify` or skip hooks.

### 5. Push and re-check

```bash
git push
```

- Invoke [worktree_ci-check](../worktree_ci-check/SKILL.md) to poll until CI completes again.

### 6. Handle repeated failures

Track the attempt count across the ci-check → ci-fix loop:

- **Attempt 2**: Return to step 1 with fresh investigation. Do not stack fixes — if the previous fix didn't work, understand why before trying again.
- **Attempt 3**: If still failing, use [worktree_ask](../worktree_ask/SKILL.md) to post a question on the issue. Include the 3 diagnosis comments as context.

If the failure is in a workflow you didn't modify, it may be a flaky test or upstream issue — report it via `worktree_ask` rather than attempting to "fix" it.

## Delegation

The following steps SHOULD be delegated to reduce token consumption:

- **Steps 1, 4** (precondition check, investigate): Spawn a Task subagent with `model: "fast"` that validates the branch name, executes `gh run list` and `gh run view --log-failed`, and returns: issue number, failing workflow/job/step, full error log.
- **Step 3** (post diagnosis comment): Spawn a Task subagent with `model: "fast"` that takes the formatted diagnosis content and posts it via `gh api`. Returns: comment URL.
- **Step 5** (push and re-check): Spawn a Task subagent with `model: "fast"` that executes `git push` and then invokes `worktree_ci-check`. Returns: push confirmation, CI check status.

Steps 2 and 4 (analyze root cause, fix) should remain in the main agent as they require debugging and code changes.

Step 6 (handle repeated failures) should remain in the main agent as it requires state tracking and escalation logic.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Delegation

The following steps SHOULD be delegated to reduce token consumption:

- **Steps 1, 4** (precondition check, investigate): Spawn a Task subagent with `model: "fast"` that validates the branch name, executes `gh run list` and `gh run view --log-failed`, and returns: issue number, failing workflow/job/step, full error log.
- **Step 3** (post diagnosis comment): Spawn a Task subagent with `model: "fast"` that takes the formatted diagnosis content and posts it via `gh api`. Returns: comment URL.
- **Step 5** (push and re-check): Spawn a Task subagent with `model: "fast"` that executes `git push` and then invokes `worktree:ci-check`. Returns: push confirmation, CI check status.

Steps 2 and 4 (analyze root cause, fix) should remain in the main agent as they require debugging and code changes.

Step 6 (handle repeated failures) should remain in the main agent as it requires state tracking and escalation logic.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Important Notes

- Never guess the cause. Always fetch the actual error log first.
- Never use `--no-verify` or skip hooks to work around a CI failure.
- Each diagnosis comment is a traceable record — future readers can follow the debugging history.
- Keep fixes atomic. One root cause, one fix, one push.
