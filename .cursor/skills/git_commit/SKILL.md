---
name: git_commit
description: Executes the commit workflow following the project's commit message conventions.
disable-model-invocation: true
---

# Git Commit Workflow

Execute the commit workflow following the project's commit message conventions.

## Workflow Steps

1. **Get staged changes context** with this command:

   ```bash

git status && echo "=== STAGED CHANGES ===" && git diff --cached

   ```

2. **Analyze the output** to understand:
- What files are staged vs un-staged
- Change types and scope (additions/deletions)
- Which changes will actually be committed
- Break down into smaller commits if no common type and scope

3. **Write accurate commit message** based on staged changes only:
- Follow rules in [commit-messages.mdc](../../rules/commit-messages.mdc)
- Include details in list form if helpful for larger commits

4. **Present the commit for review** using exactly this format:

   ````

   commit msg:

   ```
   type(scope): short description

   Refs: #<issue>
   ```

   ```bash
   git commit -m "type(scope): short description" -m "Refs: #<issue>"
   ```

   Shall I commit?

   ````

   - First block: the human-readable commit message
   - Second block: the copy-pasteable `git commit` command the user can run/edit themselves
   - No other output — no summaries, no explanations, no file lists
   - Wait for user confirmation before executing the commit

## Important Notes

- Generate minimum output; the user only needs the commit message, the command, and the confirmation prompt
- Do not read/summarize git command output after execution unless asked
- Your shell is already at the project root so you do not need `cd` or 'bash', just use `git ...`
- Do not use `--no-verify` to cheat
- Do not add any trailer (e.g. `Co-authored-by: ...`)
- **NEVER add 'Co-authored-by: Cursor <cursoragent@cursor.com>'** to commit messages.
