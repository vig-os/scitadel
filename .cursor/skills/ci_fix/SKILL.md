---
name: ci_fix
description: Diagnoses and fixes a failing CI run using systematic debugging.
disable-model-invocation: true
---

# Fix CI Failure

Diagnose and fix a failing CI run using systematic debugging.

## Workflow Steps

### 1. Get failure details

```bash
gh run list --branch $(git branch --show-current) --limit 5
gh run view <run-id> --log-failed
```

- Identify the failing workflow, job, and step.

### 2. Read the workflow file

- Open the relevant workflow in `.github/workflows/` or action in `.github/actions/`.
- Make sure you are using the correct branch specified in the workflow run details.
- Understand what the failing step does and what it depends on.

### 3. Root cause analysis (no guessing)

- **Read the error message carefully** — line numbers, file paths, exit codes.
- **Check recent changes** — `git log --oneline -10` — what changed that could cause this?
- **Compare with last passing run** — is this a new failure or pre-existing?
- **Trace the data flow** — what inputs does the failing step receive? Are they correct?

### 4. Form hypothesis and test

- State clearly: "I think X is the root cause because Y."
- Make the **smallest** change to test the hypothesis.
- Push and check CI, or reproduce locally if possible (`just test`, `just lint`, `just precommit`).

### 5. If fix doesn't work

- Do not stack more fixes. Return to step 3.
- After 3 failed attempts, question the approach and discuss with the user.

## Important Notes

- Never guess. Always fetch the actual error log first.
- Never use `--no-verify` or skip hooks to work around a CI failure.
- If the failure is in a workflow you didn't modify, it may be a flaky test or upstream issue — report it rather than "fixing" it.
