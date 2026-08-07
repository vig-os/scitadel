<!-- Managed by vigOS devkit — regenerated on upgrade; local edits are lost. -->
<!-- Customize in justfile.project. Bugs / missing tools: https://github.com/vig-os/devkit/issues -->

# Commit Message Standard

This document defines the commit message format used in this repository. Commit messages are treated as **quality records** and must follow this standard to ensure consistency, traceability, and compliance with QMS expectations (e.g. IEC 62304, ISO 13485, MDR change control).

## Format

```text
type(scope)!: short description

Body

Refs: <IDs>
```

- **First line:** `type(scope)!: short description`
  - `type` — One of the approved commit types (see below).
  - `(scope)` — Optional. Scope in parentheses (e.g. `(ci)`, `(docs)`). Alphanumeric and hyphens only; there is no fixed vocabulary — pick the one that names the subsystem you touched.
  - `!` — Optional. Indicates a breaking change.
  - `short description` — Brief, imperative summary. No period at the end.
- **Blank line** — Required after the subject.
- **Body** — Optional. Include additional context on _what_ and _why_. May have multiple paragraphs. If present, end the body with a blank line before the Refs line.
- **Refs line** — Mandatory for most types. Exactly one line starting with `Refs:`; it must be the last non-empty line. Include at least one GitHub issue ID (e.g. `#36`); other references (e.g. `REQ-...`, `RISK-...`, `SOP-...`) may follow. See [Exemptions](#exemptions) for types where `Refs:` is optional.

## Enforcing the template in VS Code

- **Git commit template** — A `.gitmessage` file in the repo root is used as the default message when you run `git commit` from the terminal (no `-m`). After `just init` or devcontainer setup, `commit.template` is set to `.gitmessage` so the template is loaded when Git opens the editor.
- **Source Control + AI** — When using the Source Control panel and the GitHub extension to generate the message:
  1. Type `committemplate` in the message box and choose the **Commit message template** snippet to insert the structure, then edit or paste the AI suggestion into the first line and `Refs:` line; or
  2. Run `git commit` from the terminal (no `-m`) so Git opens the editor with the template, then paste or adapt the AI-generated message there.
- **Validation** — The commit-msg hook (and CI) will reject any commit that does not match this standard, so the template and snippet only help you start from the right format; the hook enforces it.

## Approved commit types

Only the following types are allowed:

| Type       | Use for |
|------------|---------|
| `feat`     | New functionality, enhancements |
| `fix`      | Bug fixes |
| `docs`     | Documentation only |
| `chore`    | Maintenance, tooling, config (no code/docs behavior change) |
| `refactor` | Code refactoring, no new behavior |
| `perf`     | Performance improvements, no behavior change |
| `test`     | Tests, test infrastructure |
| `ci`       | CI/CD, workflows, automation |
| `build`    | Commits that affect build components like build scripts and makefiles |
| `revert`   | Reverting a previous commit |
| `style`    | Formatting, whitespace (no code change) |

Any other type (e.g. `feature`, `bugfix`, or emoji-based prefixes) is **not** allowed.

## Refs line and traceability

The `Refs:` line provides mandatory traceability to issues, requirements, risks, or SOPs. Only one `Refs:` line is allowed; it must be the last non-empty line of the message.

**Accepted reference formats:**

- **Issue numbers:** `#36`, (GitHub issue ID).
- **Requirements:** `REQ-123`, `REQ-SYS-001` (alphanumeric and hyphens after `REQ-`).
- **Risks:** `RISK-45`, `RISK-H-02` (alphanumeric and hyphens after `RISK-`).
- **SOPs:** `SOP-001`, `SOP-DEV-02` (alphanumeric and hyphens after `SOP-`).

**Examples of valid Refs lines:**

```text
Refs: #36
Refs: #36, #37
Refs: #36, REQ-123
Refs: #36, RISK-H-02, SOP-DEV-02
```

At least one reference to a GitHub issue must be present (e.g. `#36`).
Multiple references are comma-separated; spaces after commas are optional. Do not add a second `Refs:` line.

## Exemptions

The following commit types have a relaxed `Refs:` requirement:

- **`chore`** — The `Refs:` line is **optional**. Maintenance commits (e.g. dependency bumps, sync operations, tooling updates) may not relate to a specific issue. When a related issue or PR exists, including `Refs:` is still recommended.

Additionally, the CI validator skips two classes of commit outright:

- **Bot-authored commits** — Any commit whose author is a GitHub bot account (a name ending in `[bot]`: Renovate, Dependabot, `commit-action-bot`, …). These bots emit `build(pip): lock file maintenance` or `ci(actions): bump actions/checkout` and cannot know an issue number, so the `Refs:` requirement is waived for them regardless of type. The exemption is keyed on the author, so the same message from a human is still rejected.
- **Merge commits** — Their subject is the pull request title, which is validated separately (see below).

## Where the standard is enforced

- **Locally** — the `commit-msg` hook (via `core.hooksPath` → `.githooks/commit-msg` → `prek run --hook-stage commit-msg`). Both sanctioned environments wire `core.hooksPath` → `.githooks` automatically: the devcontainer sets it during setup, and the direnv / `nix develop` dev-shell sets it on shell entry (guarded to the main worktree). So once you have entered the dev environment the guard is active. It is only silently absent before you have entered it (or in an ad-hoc checkout outside both modes), where `core.hooksPath` may be unset and git runs no hooks.
- **In CI** — the `commit-checks` job runs `validate-commit-range`, which re-validates every commit the pull request adds (from the merge-base with the base branch) plus the **pull request title**. The title matters because pull requests merge `--no-ff`, so it becomes the merge commit's subject in the base branch's history.

Because `commit-msg` is a stage-gated hook, `prek run --all-files` does **not** run it — CI enforcement comes from the `commit-checks` job, not from the lint lane.

## Compliance note

This standard supports:

- **IEC 62304** — Software configuration and change management (traceability of changes).
- **ISO 13485** — Documented procedures and records (this document is the procedure; commit messages are records).
- **MDR / FDA** — Change control and audit trail expectations.

Enforcement is performed locally (commit-msg hook) and in CI (the `commit-checks` job) so that only compliant messages are accepted.
Existing history is not modified; the standard applies only to new commits.
