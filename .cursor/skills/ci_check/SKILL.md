---
name: ci_check
description: Checks the CI pipeline status for the current branch or PR.
disable-model-invocation: true
---

# Check CI Status

Check the CI pipeline status for the current branch or PR.

## Workflow Steps

### 1. Identify context

- If on a branch with an open PR: `gh pr checks`
- If no PR exists: `gh run list --branch $(git branch --show-current) --limit 5`

### 2. Show status per workflow

Report each workflow's status:

```
CI Status for <branch/PR>:
- CI: ✓ pass / ✗ fail / ○ pending
- CodeQL: ✓ pass / ✗ fail / ○ pending
- Scorecard: ✓ pass / ✗ fail / ○ pending
- Security Scan: ✓ pass / ✗ fail / ○ pending
```

### 3. On failure

- Show the failing job name and step.
- Run `gh run view <run-id> --log-failed` to fetch the failure log.
- Summarize the error (first relevant error line, not the full log).
- Suggest next steps: fix locally, or use [ci_fix](../ci_fix/SKILL.md) for deeper diagnosis.

## Delegation

All steps in this skill are CLI commands and output formatting, making them ideal for lightweight delegation:

Spawn a Task subagent with `model: "fast"` that:
1. Identifies the context (PR or branch) via `gh pr checks` or `gh run list`
2. Fetches the status of all workflows
3. Formats the status report with ✓/✗/○ indicators
4. For any failures, fetches the failure log via `gh run view --log-failed` and extracts the key error lines

Returns: formatted CI status report, failure logs (if any), suggested next steps.

This skill is entirely data-gathering and formatting, making it ideal for lightweight delegation.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Delegation

All steps in this skill are CLI commands and output formatting, making them ideal for lightweight delegation:

Spawn a Task subagent with `model: "fast"` that:
1. Identifies the context (PR or branch) via `gh pr checks` or `gh run list`
2. Fetches the status of all workflows
3. Formats the status report with ✓/✗/○ indicators
4. For any failures, fetches the failure log via `gh run view --log-failed` and extracts the key error lines

Returns: formatted CI status report, failure logs (if any), suggested next steps.

This skill is entirely data-gathering and formatting, making it ideal for lightweight delegation.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Important Notes

- If CI is still running, report "pending" and suggest waiting or checking back.
- Do not guess the cause of a failure. Fetch the actual log.
