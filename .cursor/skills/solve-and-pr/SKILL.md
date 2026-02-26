---
name: solve-and-pr
description: Launches the autonomous worktree pipeline for an issue via just worktree-start.
disable-model-invocation: true
---

# Solve and PR (Autonomous Launcher)

Launch the autonomous worktree pipeline for an issue. This skill acts as a bridge between your interactive editor session and the autonomous agent that runs in an isolated worktree.

**Use this when:** you want the agent to autonomously handle design, planning, implementation, verification, PR creation, and CI — all without further human interaction.

## Workflow Steps

### 1. Validate issue number

- The user provides an issue number (e.g. `/solve-and-pr 42`).
- Confirm the issue exists: `gh issue view <issue_number> --json number,title`

### 2. Launch the worktree

```bash
just worktree-start <issue_number> "/worktree-solve-and-pr"
```

This command:

- Creates (or reuses) a git worktree for the issue
- Resolves or creates the linked branch
- Sets up the environment (`uv sync`, `pre-commit install`)
- Captures the local gh user as the reviewer (`gh api user --jq '.login'`)
- Launches a tmux session running `cursor-agent` with `--yolo` mode
- Passes `/worktree-solve-and-pr` as the initial prompt

### 3. Report back to the user

After `just worktree-start` completes, tell the user:

```text
Worktree launched for issue #<issue_number>

The autonomous agent is running in the background. Progress will be posted as comments on the issue.

Commands:
  Attach (watch): just worktree-attach <issue_number>
  List all:       just worktree-list
  Stop:           just worktree-stop <issue_number>

The agent will:
  1. Design (posts ## Design comment)
  2. Plan (posts ## Implementation Plan comment)
  3. Execute (commits code)
  4. Verify (runs tests, lint, precommit)
  5. Create PR (you as reviewer)
  6. Wait for CI (auto-fix on failure)

Check the issue for updates: https://github.com/<owner>/<repo>/issues/<issue_number>
```

## Important Notes

- This is a **fire-and-forget** launcher. The skill returns immediately after launching the worktree. It does not wait for the autonomous run to complete.
- The autonomous agent runs in a separate tmux session. You can attach to watch it (`just worktree-attach <issue>`), but it does not require your input.
- The local gh user (the person who invoked this skill) is set as the PR reviewer via the `WORKTREE_REVIEWER` environment variable.
- If the worktree already exists and a tmux session is running, `just worktree-start` will report that and you can use `just worktree-attach` instead.
- All progress is visible as issue comments with H2 headings: `## Design`, `## Implementation Plan`, `## CI Diagnosis`, etc.
