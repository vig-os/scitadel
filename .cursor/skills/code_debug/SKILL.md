---
name: code_debug
description: Diagnoses bugs, test failures, or unexpected behavior. Root cause first, fix second.
disable-model-invocation: true
---

# Systematic Debugging

Diagnose bugs, test failures, or unexpected behavior. Root cause first, fix second.

**Rule: no fixes without root cause investigation.**

## Workflow Steps

### Phase 1: Investigate

1. **Read error messages** — full stack traces, line numbers, exit codes. Don't skip past them.
2. **Reproduce** — can you trigger it reliably? What are the exact steps?
3. **Check recent changes** — `git diff`, recent commits, new dependencies, config changes.
4. **Trace data flow** — where does the bad value originate? Trace backward through the call stack until you find the source.

### Phase 2: Analyze

1. **Find working examples** — locate similar working code in the codebase.
2. **Compare** — what's different between working and broken?
3. **Check dependencies** — settings, config, environment assumptions.

### Phase 3: Hypothesize and test

1. **Form one hypothesis** — "I think X is the root cause because Y."
2. **Test minimally** — smallest possible change to test the hypothesis. One variable at a time.
3. **Evaluate** — did it work? If not, form a new hypothesis. Do not stack fixes.

### Phase 4: Fix

1. **Write a failing test** that reproduces the bug (following [code_tdd](../code_tdd/SKILL.md)).
2. **Implement the fix** — address root cause, not symptoms. One change.
3. **Verify** — test passes, no regressions.
4. **If 3+ fixes failed** — stop. Question the architecture. Discuss with the user.

## Stop If

- You are about to propose a fix without completing Phase 1.
- You are stacking a second fix on top of a failed first fix.
- You are thinking "just try this and see if it works."
- You have tried 3+ fixes without success (architectural problem — discuss with user).
