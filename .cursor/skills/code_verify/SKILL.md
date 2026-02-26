---
name: code_verify
description: Runs verification and provides evidence before claiming work is done.
disable-model-invocation: true
---

# Verify Before Completion

Run verification and provide evidence before claiming work is done.

**Rule: no "should work" or "looks correct". Evidence only.**

## Workflow Steps

### 1. Identify what to verify

- What claim are you about to make? (tests pass, build works, bug fixed, feature complete)
- What command proves it?

### 2. Run verification

```bash
just test              # full test suite
just test-image        # or specific suite
just lint              # linters
just precommit         # pre-commit hooks on all files
```

- Run the **full** command. Do not rely on partial output or previous runs.

### 3. Read output and confirm

- Check exit code.
- Count failures/warnings.
- If output confirms the claim → state the claim with evidence.
- If output contradicts the claim → state the actual status with evidence.

### 4. Report

```
Verification: <what was checked>
Command: <what was run>
Result: <pass/fail with key output>
```

## Stop If

- You are about to say "should pass", "looks correct", "seems fine", or "done".
- You haven't run the verification command in this message.
- You are relying on a previous run or partial check.
- You are trusting a subagent's success report without independent verification.
