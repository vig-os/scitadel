---
name: worktree_brainstorm
description: Autonomous design — reads full issue, posts design comment, never blocks for feedback.
disable-model-invocation: true
---

# Autonomous Brainstorm

Explore requirements and produce a design **without user interaction**. This is the worktree variant of [design_brainstorm](../design_brainstorm/SKILL.md) — it makes reasonable decisions autonomously instead of asking the user.

**Rule: no code until a design is posted. No blocking for feedback.**

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
- Parse **comments** for prior discussion, existing design (`## Design` heading), or context.
- If a `## Design` comment already exists, **skip** — the design phase is done.

### 2. Explore project context

- Read relevant files, docs, recent commits to understand current state.
- Identify constraints, existing patterns, and related code.

### 3. Make design decisions autonomously

- Where the interactive variant would ask clarifying questions, make a reasonable choice based on:
  - The issue body's proposed solution (treat it as the user's intent).
  - Existing project patterns and conventions.
  - YAGNI — when in doubt, choose the simpler option.
- Document each decision and the rationale.

### 4. Produce design

- Write the design covering: architecture, components, data flow, error handling, testing strategy.
- Scale to complexity — a simple issue gets a few sentences, a complex one gets sections.

### 5. Publish design as a GitHub issue comment

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Post the design comment:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/comments \
     -f body="<design_content>"
   ```

3. The comment **must** start with `## Design` (H2) — this is how other skills detect that the design phase is complete.

### 6. Proceed to planning

- Invoke [worktree_plan](../worktree_plan/SKILL.md) to break the design into tasks.

## Delegation

The following steps SHOULD be delegated to reduce token consumption:

- **Steps 1, 4** (precondition check, read issue): Spawn a Task subagent with `model: "fast"` that validates the branch name, executes `gh issue view`, and checks for an existing `## Design` comment. Returns: issue number, parsed body/comments, design-exists flag.
- **Step 5** (publish design): Spawn a Task subagent with `model: "fast"` that takes the formatted design content and posts it via `gh api`. Returns: comment URL.
- **Step 6** (invoke next skill): Can remain in main agent (simple skill invocation).

Steps 2-3 (explore context, make design decisions) should remain in the main agent as they require architectural reasoning and decision-making.

Reference: [subagent-delegation rule](../../rules/subagent-delegation.mdc)

## When stuck

If you cannot make a reasonable design decision (genuinely ambiguous, high-risk, or contradictory requirements), use [worktree_ask](../worktree_ask/SKILL.md) to post a question on the issue. Do not guess on critical decisions.

## Important Notes

- Never block waiting for user input. Make decisions, document rationale, move on.
- The issue body is the primary input — treat its proposed solution as the user's preferred direction.
- The design can be short for simple issues. It must exist as an issue comment.
