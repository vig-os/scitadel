---
name: pr_solve
description: Diagnoses all PR failures (CI, reviews, merge state), plans fixes, executes them.
disable-model-invocation: true
---

# Solve PR Failures

Diagnose all failures on a pull request — CI failures, review feedback, merge conflicts — produce a consolidated fix plan, and execute it.

**Rule: no fixes without presenting the diagnosis first. No guessing — cite actual output.**

## When to Use

- A PR has failing CI checks, requested changes from reviewers, or merge conflicts.
- You want a single entry point that gathers all problems, plans fixes, and executes them — instead of manually orchestrating [ci_check](../ci_check/SKILL.md), review reading, and [ci_fix](../ci_fix/SKILL.md) individually.

## Workflow Steps

### 1. Identify the PR

- The user provides a PR number (e.g. `/pr_solve 42`).
- Fetch PR metadata:

  ```bash
  gh pr view <number> --json number,title,body,headRefName,baseRefName,mergeable,mergeStateStatus,reviewDecision,state
  ```

- Derive the linked issue number from the PR body (`Closes #N`, `Refs: #N`, or `Fixes #N`). If no issue is linked, ask the user.
- Confirm the PR is open. If merged or closed, stop and tell the user.

### 2. Gather all problems

Collect problems from three independent sources. Keep them separated — they are different concerns that require different fixes.

#### 2a. CI failures

```bash
gh pr checks <number>
```

- For each failing check, fetch the failure log:

  ```bash
  gh run view <run-id> --log-failed
  ```

- Extract: workflow name, job, step, key error lines.
- If all checks pass or are pending, note it and move on.

#### 2b. Review feedback

```bash
gh api repos/{owner}/{repo}/pulls/<number>/reviews \
  --jq '[.[] | select(.state == "CHANGES_REQUESTED" or .state == "COMMENTED") | {author: .user.login, state: .state, body: .body}]'
```

```bash
gh api repos/{owner}/{repo}/pulls/<number>/comments \
  --jq '[.[] | {author: .user.login, path: .path, line: .line, body: .body, url: .html_url}]'
```

- Include only unresolved review threads (comments without a resolution).
- Group by reviewer, then by file.
- If no pending reviews or comments, note it and move on.

#### 2c. Merge state

- From step 1's metadata, check `mergeable` and `mergeStateStatus`.
- If there are merge conflicts, list the conflicting status but **do not attempt an automatic rebase** — report it as requiring manual resolution.

### 3. Present diagnosis

Show the user a structured summary before any fixes:

```
## PR Diagnosis: #<number>

### CI Failures
- <workflow> / <job> / <step>: <key error line> (run <run-id>)
- ...
(or: All CI checks passing ✓)

### Review Feedback
- @<reviewer> (changes requested):
  - `<file>:<line>`: <comment summary> ([link](<url>))
  - ...
(or: No pending review feedback ✓)

### Merge State
- <mergeable status>
(or: Clean — no conflicts ✓)
```

**If no problems are found in any category**, report a clean bill of health and stop. Do not proceed to planning.

**Wait for the user to acknowledge the diagnosis before proceeding.**

### 4. Plan fixes

- For each problem, create an ordered fix task following [design_plan](../design_plan/SKILL.md) conventions:
  - **What**: one sentence describing the fix
  - **Files**: exact file paths to modify
  - **Verification**: how to confirm the fix works
- Order: CI failures first (they block merge), then review feedback (by file to minimize context switching), then merge conflicts last (manual).
- Merge conflicts are listed as "manual action required" — the skill does not rebase.
- Present the plan to the user for approval. Do not start fixing until approved.

### 5. Execute fixes

- **Merge the base branch** before the first push: run `git fetch origin` and `git merge origin/<base_branch>` (use `baseRefName` from step 1's PR metadata). **Conflict handling:** If merge conflicts occur, list the conflicting files and ask the user to resolve them before proceeding.
- Work through approved tasks one at a time.
- Follow [code_tdd](../code_tdd/SKILL.md) discipline where applicable (write test first, then fix).
- Commit each fix via [git_commit](../git_commit/SKILL.md).
- Push after each fix: `git push`

### 6. Verify

- After all fixes are pushed, run [ci_check](../ci_check/SKILL.md) to confirm CI passes.
- If new failures appear, loop back to step 2 to re-diagnose.
- **Maximum 2 loops.** After the second re-diagnosis, escalate to the user — do not keep cycling.

## Delegation

Step 2 (gather all problems) is entirely data-gathering and CLI commands, making it ideal for lightweight delegation:

Spawn a Task subagent with `model: "fast"` that:
1. Runs `gh pr checks` and fetches `--log-failed` for any failing runs
2. Fetches reviews and inline comments via `gh api`
3. Extracts merge state from the PR metadata

Returns: structured data for each category (CI failures with error logs, review comments grouped by reviewer/file, merge state).

Steps 3-6 (diagnosis presentation, planning, execution, verification) remain in the main agent as they require user interaction and code changes.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Stop If

- You are about to fix something without presenting the diagnosis (step 3) first.
- You are guessing at the cause of a CI failure — fetch the log.
- You are attempting a rebase or merge conflict resolution automatically.
- You are stacking a second fix on top of a failed first fix — re-diagnose instead.
- You have looped through steps 2-6 more than twice — escalate to the user.
