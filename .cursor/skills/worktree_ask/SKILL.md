---
name: worktree_ask
description: Posts a question to the GitHub issue when the autonomous agent is stuck.
disable-model-invocation: true
---

# Ask for Help

Post a question on the GitHub issue when the autonomous agent cannot make a reasonable decision. **Placeholder implementation** — a future issue will add Telegram/Element bot integration for push notifications.

## When to Use

- A design decision is genuinely ambiguous, high-risk, or contradictory.
- A verification failure persists after 3 fix attempts.
- The issue body or existing comments contain conflicting requirements.

Do **not** use this for routine decisions — make a reasonable choice and document the rationale instead.

## Workflow Steps

### 1. Formulate the question

- State what you're trying to do.
- State what's blocking you (the ambiguity, conflict, or failure).
- Propose 2-3 options if applicable.
- Keep it concise — the user will read this on their phone.

### 2. Post as an issue comment

1. Determine the repo: `gh repo view --json nameWithOwner --jq '.nameWithOwner'`
2. Post the question:

   ```bash
   gh api repos/{owner}/{repo}/issues/{issue_number}/comments \
     -f body="<question_content>"
   ```

3. The comment **must** start with `## Question` (H2) so it's identifiable.
4. Format:

   ```markdown
   ## Question

   **Context:** <what phase you're in, what you're trying to do>

   **Blocker:** <what's preventing progress>

   **Options:**
   1. Option A — <trade-off>
   2. Option B — <trade-off>

   Please reply to this comment with your preference.
   ```

### 3. Poll for reply (placeholder)

Currently, there is no push notification mechanism. The agent should:

1. Log that a question was posted and pause the current phase.
2. Wait for a configurable timeout (default: 5 minutes).
3. Re-fetch issue comments and check for a reply after the `## Question` comment.
4. If a reply is found, parse it and resume.
5. If no reply after timeout, make the safest choice (Option A or the simplest option) and document that the decision was made autonomously due to timeout.

### Future: Telegram/Element bot

A future issue will replace the polling mechanism with:
- Push notification to Telegram/Element when a question is posted.
- Bot API endpoint that receives the reply and unblocks the agent.
- See related discussion in issue #64.

## Important Notes

- This is a last resort. Prefer making autonomous decisions with documented rationale.
- Keep questions focused and actionable — yes/no or multiple choice.
- Always provide a default option so the timeout fallback is safe.
