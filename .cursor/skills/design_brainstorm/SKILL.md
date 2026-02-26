---
name: design_brainstorm
description: Explores requirements and design before writing any code.
disable-model-invocation: true
---

# Brainstorm

Explore requirements and design before writing any code. This command activates before creative work — features, components, behavior changes.

**Rule: no code until the user approves a design.**

## Precondition: Issue Branch Required

Before doing anything else, verify you are on an issue branch:

1. Run: `git branch --show-current`
2. The branch name **must** match `<type>/<issue_number>-*` (e.g. `feature/63-worktree-support`).
3. Extract the `<issue_number>` from the branch name.
4. If the branch does not match, **stop** and tell the user:
   - They need to be on an issue branch.
   - Offer to run [issue_claim](../issue_claim/SKILL.md) to create one.

## Workflow Steps

### 1. Explore project context

- Read relevant files, docs, recent commits to understand current state.
- Identify constraints, existing patterns, and related code.
- Check issue comments for prior discussion or context.

### 2. Ask clarifying questions

- One question at a time. Do not overwhelm.
- Prefer multiple choice when possible; open-ended is fine when needed.
- Focus on: purpose, constraints, success criteria, edge cases.
- Continue until you understand the full scope.

### 3. Propose approaches

- Present 2-3 approaches with trade-offs.
- Lead with your recommended option and explain why.
- Apply YAGNI — cut anything speculative.

### 4. Present design for approval

- Present the design in sections, scaled to complexity.
- After each section, ask: "Does this look right so far?"
- Cover: architecture, components, data flow, error handling, testing strategy.
- Revise if the user pushes back. Go back to questions if something is unclear.

### 5. Publish design as a GitHub issue comment

After user approval, post the design as a **comment on the issue**. This is the durable, visible record.

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Post the design comment:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/comments \
     -f body="<design_content>"
   ```

3. The comment must start with `## Design` (H2) so other skills can detect the design phase is complete.

### 6. Transition to planning

- Hand off to the [design_plan](../design_plan/SKILL.md) skill to break the design into implementation tasks.

## Important Notes

- **Do not run** without being on an issue branch. No exceptions.
- Every project goes through this, regardless of perceived simplicity. The design can be short (a few sentences) for truly simple tasks, but it must exist and be approved.
- Do not invoke any implementation command or write any code until design is approved.
- If the user says "just do it" or "skip design", push back once explaining why, then comply if they insist.
