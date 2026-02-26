---
name: worktree_ci-check
description: Autonomous CI check — polls until CI finishes, invokes worktree_ci-fix on failure.
disable-model-invocation: true
---

# Autonomous CI Check

Poll CI pipeline status and react **without user interaction**. This is the worktree variant of [ci_check](../ci_check/SKILL.md) — it waits for CI to finish and auto-triggers fixes instead of reporting status and stopping.

**Rule: no blocking for feedback. Poll until resolution.**

## Precondition: Issue Branch Required

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-<summary>` (e.g. `feature/79-declarative-sync-manifest`). See [branch-naming.mdc](../../rules/branch-naming.mdc) for the full convention.
3. Extract the `<issue_number>` from the branch name.
4. If the branch does not match, **stop** and log the error.

## Workflow Steps

### 1. Identify the PR

```bash
gh pr list --head $(git branch --show-current) --json number,url --jq '.[0]'
```

- If a PR exists, use `gh pr checks <number>` for status.
- If no PR exists, use `gh run list --branch $(git branch --show-current) --limit 5`.

### 2. Poll until CI completes

Check status with exponential backoff:

1. Wait **30 seconds** (initial delay — give CI time to start).
2. Run `gh pr checks <number>` (or `gh run list ...`).
3. If any check is still pending:
   - Wait with backoff: 30s → 60s → 120s → 120s (cap).
   - Re-check after each wait.
   - Maximum total wait: **15 minutes**. If still pending after 15 minutes, post a note via [worktree_ask](../worktree_ask/SKILL.md) and stop.
4. If all checks pass → proceed to completion (step 4).
5. If any check fails → proceed to failure handling (step 3).

### 3. Handle failure

On CI failure:

1. Identify the failing workflow, job, and step from `gh pr checks` output.
2. Fetch the failure log:

   ```bash
   gh run view <run-id> --log-failed
   ```

3. Invoke [worktree_ci-fix](../worktree_ci-fix/SKILL.md) with the failure context.

### 4. Report success

Once all checks pass, log the result:

```
CI Status: all checks pass
- <workflow_name>: pass
- <workflow_name>: pass
...
```

No comment is posted on success — the green CI status on the PR is sufficient.

## Delegation

The following steps SHOULD be delegated to reduce token consumption:

- **Steps 1-2** (precondition check, identify PR, poll CI): Spawn a Task subagent with `model: "fast"` that validates the branch name, identifies the PR via `gh pr list`, and polls `gh pr checks` with exponential backoff until completion or 15-minute timeout. Returns: issue number, PR number/URL, final CI status for all checks.
- **Step 4** (report success): Can remain in main agent (simple logging).

Step 3 (handle failure) should remain in the main agent as it requires log analysis and invoking the ci-fix skill with context.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Important Notes

- Never guess CI status. Always fetch it via `gh`.
- If CI hasn't started yet (no runs found), wait and re-check — the run may take a moment to appear after push.
- If the PR was just created, allow extra time for workflows to trigger.
