---
name: worktree_solve-and-pr
description: State-aware autonomous pipeline — detect phase from issue, run remaining phases through PR.
disable-model-invocation: true
---

# Solve and PR

Autonomous end-to-end pipeline that reads the full issue to determine what's already done, then runs the remaining phases through to a pull request.

**Rule: no blocking for feedback. Detect state, resume from where things left off.**

## Precondition: Issue Branch Required

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-<summary>` (e.g. `feature/79-declarative-sync-manifest`). See [branch-naming.mdc](../../rules/branch-naming.mdc) for the full convention.
3. Extract the `<issue_number>` from the branch name.
4. If the branch does not match, **stop** and log the error.

## Workflow Steps

### 1. Read the full issue

```bash
gh issue view <issue_number> --json title,body,labels,comments
```

- Parse the **body** for: description, proposed solution, acceptance criteria, constraints.
- Parse **comments** for completed phase markers (H2 headings).

### 2. Detect current state

Scan issue comments for these H2 headings:

| Comment heading found          | Phase complete | Next phase        |
|-------------------------------|----------------|-------------------|
| *(none)*                       | —              | `worktree_brainstorm` |
| `## Design`                   | Design         | `worktree_plan`   |
| `## Implementation Plan`      | Planning       | `worktree_execute` |

The issue body is **always** read as the foundation — it contains the problem, proposed solution, and acceptance criteria. Comments layer completed phases on top.

### 3. Run remaining phases

Execute phases in order, starting from the detected state:

1. **Design** → [worktree_brainstorm](../worktree_brainstorm/SKILL.md)
   - Reads issue body, explores context, posts `## Design` comment.
2. **Plan** → [worktree_plan](../worktree_plan/SKILL.md)
   - Reads issue body + design, posts `## Implementation Plan` comment.
3. **Execute** → [worktree_execute](../worktree_execute/SKILL.md)
   - Implements tasks from the plan, TDD, commits after each task.
4. **Verify** → [worktree_verify](../worktree_verify/SKILL.md)
   - Full test suite + lint + precommit. Loops back to fix on failure.
5. **PR** → [worktree_pr](../worktree_pr/SKILL.md)
   - Creates pull request with auto-generated text.
6. **CI** → [worktree_ci-check](../worktree_ci-check/SKILL.md)
   - Polls remote CI until completion. On failure, invokes [worktree_ci-fix](../worktree_ci-fix/SKILL.md) which diagnoses, fixes, pushes, and loops back to ci-check.

Each phase checks for its own completion marker before running. If the marker exists, it skips to the next phase.

### 4. Report completion

After the PR is created, post a summary comment on the issue:

```markdown
## Autonomous Run Complete

- Design: posted
- Plan: posted (<n> tasks)
- Execute: all tasks done
- Verify: all checks pass
- PR: <PR_URL>
- CI: all checks pass
```

## Delegation

The following steps SHOULD be delegated to reduce token consumption:

- **Steps 1-2** (precondition check, read issue, detect state): Spawn a Task subagent with `model: "fast"` that runs the branch validation, executes `gh issue view`, parses the JSON output, and scans comments for H2 headings. Returns: issue number, parsed body/comments, detected phase state.
- **Step 4** (report completion): Spawn a Task subagent with `model: "fast"` that formats and posts the summary comment via `gh api`.

Step 3 (orchestration) should remain in the main agent as it requires understanding skill dependencies and phase transitions.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## Important Notes

- Never block for user input at any phase. Each sub-skill is autonomous.
- The issue body is the primary input at every phase — never ignore it.
- If any phase uses [worktree_ask](../worktree_ask/SKILL.md), the pipeline pauses until a reply is received (or timeout).
- This skill is typically invoked via `just worktree-start <issue> "<prompt>"` where the prompt references this skill.
