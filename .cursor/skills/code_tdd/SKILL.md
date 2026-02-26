---
name: code_tdd
description: Implements changes using strict RED-GREEN-REFACTOR discipline.
disable-model-invocation: true
---

# Test-Driven Development

Implement changes using strict RED-GREEN-REFACTOR discipline.
Each phase is committed separately so the git history proves TDD compliance to auditors.

## Workflow Steps

### 1. Understand what to test

- Read the issue's acceptance criteria or the current task from the plan.
- Identify the behavior to implement and the expected outcomes.
- Use the [tdd.mdc](../../rules/tdd.mdc) scenario checklist to decide which test categories apply.

### 2. Verify the suite is green

- Identify the test suite you will expand (check the `justfile` for available test recipes).
- Run it once to confirm it **passes** before adding new tests. If it fails, fix or report the existing failure first — do not proceed with a broken baseline.

### 3. RED — Write a failing test

- Write the test **before** any implementation code.
- The test must assert the expected behavior.
- Run the relevant test suite (see `justfile` for available recipes) to confirm the test **fails**.
- If the test passes before implementation, the test is wrong or the feature already exists. Investigate.

### 4. Commit the failing test

- **Commit** using [git_commit](../git_commit/SKILL.md) with type `test`, e.g. `test: add failing test for <behavior>`.
- Do **not** proceed to GREEN before this commit is created.
- This creates an auditable record that the test was written first.

### 5. GREEN — Write minimal code to pass

- Write the **smallest** amount of code that makes the failing test pass.
- Do not add extra functionality, error handling, or optimizations yet.
- Run the test again to confirm it **passes**.
- Run the full relevant test suite to confirm no regressions.
- **Commit the implementation** using [git_commit](../git_commit/SKILL.md), e.g. `feat: implement <behavior>`.

### 6. REFACTOR — Clean up

- Improve the code without changing behavior (rename, extract, simplify).
- Run tests again after refactoring to confirm nothing broke.
- **Commit the refactor** using [git_commit](../git_commit/SKILL.md) with type `refactor`, if there are meaningful changes. Skip if nothing changed.

## Important Notes

- Never write implementation code before its test.
- If you catch yourself writing code first, stop, delete the code, write the test.
- One RED-GREEN-REFACTOR cycle per behavior. Don't batch multiple behaviors.
- The commit after RED (failing test) is critical — it is the proof of TDD for regulatory/quality audits.
- If no test framework applies (e.g. pure config changes), skip TDD but note why.
