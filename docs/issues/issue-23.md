---
type: issue
state: closed
created: 2026-02-26T22:07:57Z
updated: 2026-11-18T14:59:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/23
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:39:07.063Z
---

# [Issue 23]: [[FEATURE] Conductor: Rich-based build orchestrator with phase-granular agent dispatch](https://github.com/vig-os/scitadel/issues/23)

## Problem

No automated way to orchestrate the Phase 1 build across the dependency DAG. Each issue must be manually dispatched via `just wt-start`, phases tracked by reading issue comments, and dependencies checked by hand.

## Solution

A Rich Live dashboard (`just conductor`) that:

1. Reads the dependency DAG from `Blocked by: #X` in issue bodies
2. Detects current phase per issue from GitHub comments (`## Design`, `## Implementation Plan`) and `phase:*` labels
3. Dispatches individual phase skills (`wt_brainstorm`, `wt_plan`, `wt_execute`, `wt_verify`, `wt_pr`) one at a time per issue
4. Monitors tmux sessions for activity (spinning indicator while output is being generated)
5. Kills worktrees after each phase completes, dispatches next phase
6. Auto-merges PRs after local verify + CI passes
7. Escalates to human when `## Question` is posted or agent is stuck
8. Runs inside tmux for reconnectability on remote devcontainers

## Supporting changes

- **`just issue <no>`** — structured context dump for agents (rendered markdown, token-efficient)
- **`phase:*` labels** — machine-readable phase tracking on issues
- **`just verify`** — local CI mirror (precommit + tests + security) — already added
- **Skill updates** — each `worktree_*` skill adds `phase:*` label on completion, uses `just issue` as context loader

## Architecture

- `conductor.py` in `.devcontainer/scripts/` (Rich Live, polls every ~30s)
- `just conductor [epic]` recipe
- DAG parsed from issue bodies (`Blocked by: #X, #Y`)
- Phase detection from labels (fast) with comment fallback

## State machine (per issue)

```
blocked → ready → design → plan → execute → verify → pr → review → merged
```

Each phase = one worktree agent dispatch. Observable via `phase:*` labels and `## Heading` comments.

## Acceptance criteria

- [ ] `just conductor 1` renders a Rich Live dashboard showing all Phase 1 issues with phase/status
- [ ] Conductor dispatches `wt_brainstorm` for ready issues with no design
- [ ] Conductor advances through phases as comment markers and labels appear
- [ ] Conductor kills worktree sessions between phases
- [ ] `just issue <no>` prints structured issue context to stdout
- [ ] `phase:*` labels added to taxonomy and used by skills
- [ ] Conductor auto-merges PRs after verify passes
- [ ] Conductor highlights escalations (`## Question`)
---

# [Comment #1]() by [gerchowl]()

_Posted on February 26, 2026 at 10:08 PM_

## Design (v2) — Conductor: Granular Agent Orchestration

Supersedes v1. Incorporates: micro-steps with model tiers, Pydantic structured protocol, local SQLite state, warm agent pool with tmux, file-based I/O, test-driven swarming.

### Architecture overview

Conductor is a **Python script** that orchestrates autonomous agent sessions via a **warm tmux agent pool**. It runs inside its own tmux session for reconnectability.

```
conductor.py (Python, tmux: conductor)
├── AgentPool: N warm `agent chat` tmux sessions
├── StateDB: .conductor/state.db (SQLite)
├── Step I/O: .conductor/steps/{issue}/{step}.{input,output}.json
├── Pydantic models: structured contracts between steps
└── GitHub sync: lightweight background sync of labels/comments
```

**Key principle:** The conductor does all mechanical work (context loading, file parsing, git operations, GitHub sync) in Python. Only creative work (design, architecture, test writing, code writing, debugging) is dispatched to agent sessions.

### Agent pool

Instead of cold-starting `agent --print` per step (~5-10s overhead), the conductor maintains a pool of **warm `agent chat` sessions** in tmux.

**Spin-up (once):**
```bash
tmux new-session -d -s agent-0 -c {worktree} "agent chat --yolo --trust --approve-mcps"
tmux new-session -d -s agent-1 -c {worktree} "agent chat --yolo --trust --approve-mcps"
tmux new-session -d -s agent-2 -c {worktree} "agent chat --yolo --trust --approve-mcps"
```

**Dispatch (per step):**
```bash
tmux send-keys -t agent-0 "/model opus-4.6" Enter       # switch tier
tmux send-keys -t agent-0 "/clear" Enter                 # fresh context
tmux send-keys -t agent-0 "Read .conductor/steps/5/3.2.input.json. Do work. Write result to .conductor/steps/5/3.2.output.json" Enter
```

**Model switching via `/model`** — sessions are model-agnostic. The conductor switches tiers on the fly:
- `/model opus-4.6` for autonomous steps (design, architect)
- `/model sonnet-4.5` for standard steps (test writing, code writing)
- `/model composer-1.5` for lightweight steps (if needed beyond Python)
- `/clear` between steps for fresh context
- `/compress` when context carryover is intentional (e.g., design → plan on same session)

**Dynamic scaling:**
- Start with 0 sessions
- `acquire(tier)` checks for idle session, spins up if none available
- Release after step completes (session stays warm)
- Drain idle sessions after configurable TTL (default: 60s)
- Max pool size configurable (default: 3)

### File-based I/O protocol

Agents and conductor communicate via files, not terminal output:

```
.conductor/
├── state.db                          # SQLite: issue state, step history, GH sync queue
├── steps/
│   └── {issue}/
│       ├── 1.1.input.json            # Conductor writes input
│       ├── 1.1.output.json           # Agent writes output
│       ├── 3.2.input.json
│       ├── 3.2.output.json
│       ├── 4.2.1.input.json          # Swarm task for agent 1
│       ├── 4.2.1.output.json
│       ├── 4.2.2.input.json          # Swarm task for agent 2
│       └── 4.2.2.output.json
└── .gitignore                        # Ignore everything
```

**Validation:** Conductor reads output file → `json.loads()` → `PydanticModel.model_validate()`. On failure: send retry prompt to same session with error message. Max 2 retries, then escalate.

### Pydantic structured models (inter-step protocol)

```python
class IssueContext(BaseModel):
    number: int
    title: str
    body: str
    labels: list[str]
    phase: str
    blocked_by: list[BlockerStatus]
    branch: str
    design: str | None
    plan: ImplementationPlan | None

class ImplementationPlan(BaseModel):
    tasks: list[PlanTask]

class PlanTask(BaseModel):
    id: str
    description: str
    files: list[str]
    verification: str

class TestMatrix(BaseModel):
    entries: list[TestMatrixEntry]

class TestMatrixEntry(BaseModel):
    task_id: str
    function: str
    file: str
    categories: list[TestCategory]

class TestCategory(BaseModel):
    name: str       # "happy_path", "edge_cases", etc.
    applies: bool
    reasoning: str   # WHY it applies or WHY NOT

class StubManifest(BaseModel):
    test_files: list[StubFile]
    impl_files: list[StubFile]

class StubFile(BaseModel):
    path: str
    functions: list[StubFunction]

class StubFunction(BaseModel):
    name: str
    docstring: str
    signature: str

class TestAssignment(BaseModel):
    test_file: str
    stubs: list[StubFunction]
    related_impl_stubs: list[StubFile]

class ImplAssignment(BaseModel):
    impl_file: str
    stubs: list[StubFunction]
    related_test_content: str
    test_output: str  # RED output showing what needs to pass

class FileOutput(BaseModel):
    file: str
    content: str
```

### Local SQLite state

```sql
CREATE TABLE issues (
    number INTEGER PRIMARY KEY,
    title TEXT,
    phase TEXT,
    current_step TEXT,
    dispatched_at TEXT,
    completed_at TEXT,
    blocked_by TEXT,  -- JSON array
    branch TEXT,
    pr_number INTEGER,
    stuck_reason TEXT
);

CREATE TABLE steps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_number INTEGER REFERENCES issues(number),
    step TEXT,          -- '1.1', '4.2.1', etc.
    model_tier TEXT,
    status TEXT,        -- pending/dispatched/completed/failed/retrying
    input_path TEXT,
    output_path TEXT,
    dispatched_at TEXT,
    completed_at TEXT,
    duration_ms INTEGER,
    error TEXT
);

CREATE TABLE gh_sync (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_number INTEGER,
    sync_type TEXT,     -- label_add, comment_post, etc.
    payload TEXT,
    synced_at TEXT,
    status TEXT         -- pending/synced/failed
);
```

DB is source of truth for conductor state. GitHub is a projection synced by background Python code.

### Micro-step breakdown (per issue)

#### Phase 1: DESIGN

| Step | Model | Dispatch | Description |
|---|---|---|---|
| 1.1 | Python | direct | Run `just issue <no>`, write `IssueContext` to input file |
| 1.2 | autonomous | agent pool | Read context, explore codebase, produce design |
| 1.3 | Python | direct | Parse agent output, post `## Design` comment, add `phase:design` label |

#### Phase 2: PLAN

| Step | Model | Dispatch | Description |
|---|---|---|---|
| 2.1 | Python | direct | Reload `IssueContext` (with design) |
| 2.2 | autonomous | agent pool | Break design into `ImplementationPlan` tasks |
| 2.3 | Python | direct | Post `## Implementation Plan` comment, add `phase:plan` label |

#### Phase 3: ARCHITECT

| Step | Model | Dispatch | Description |
|---|---|---|---|
| 3.1 | Python | direct | Reload context with plan |
| 3.2 | autonomous | agent pool | Produce `TestMatrix` — argue for/against each TDD category per function. Default is YES; must argue NO with rationale |
| 3.3 | autonomous | agent pool (same session, `/compress`) | Write stub files to disk: test functions with docstrings, impl functions with type signatures. Commit stubs |
| 3.4 | Python | direct | AST-parse stubs from disk into `StubManifest`. Partition into `TestAssignment` batches |

#### Phase 4: TEST (swarm)

| Step | Model | Dispatch | Description |
|---|---|---|---|
| 4.1 | Python | direct | Write `TestAssignment` input files for each batch |
| 4.2.N | standard (×N parallel) | agent pool | Each agent fills test bodies for assigned file. Runs own tests (expects RED) |
| 4.3 | Python | direct | Verify all tests fail (RED), commit test files |

#### Phase 5: IMPLEMENT (swarm)

| Step | Model | Dispatch | Description |
|---|---|---|---|
| 5.1 | Python | direct | Write `ImplAssignment` input files (includes test content + RED output) |
| 5.2.N | standard (×N parallel) | agent pool | Each agent fills implementation code. Runs related tests (expects GREEN) |
| 5.3 | Python | direct | Verify passing tests, commit per module |
| 5.4 | standard (if failures) | agent pool | Retry failed implementations (max 2, then escalate) |

#### Phase 6: VERIFY

| Step | Model | Dispatch | Description |
|---|---|---|---|
| 6.1 | Python | direct | Run `just verify`, capture results |
| 6.2 | standard (if failures) | agent pool | Diagnose and fix. Loop max 3 |
| 6.3 | Python | direct | Add `phase:verify` label |

#### Phase 7: PR

| Step | Model | Dispatch | Description |
|---|---|---|---|
| 7.1 | Python | direct | Gather git log, diff stat, issue context |
| 7.2 | standard | agent pool | Fill PR template |
| 7.3 | Python | direct | `gh pr create`, add `phase:pr` label |
| 7.4 | Python | direct | `gh pr merge --squash --auto` after CI |

### Test-driven swarming flow

```
ARCHITECT (autonomous, single agent)
    │
    ├── TestMatrix: for each function, argues YES/NO per TDD category
    ├── Writes stubs to disk (test + impl files with signatures + docstrings)
    └── Commits stubs
           │
    CONDUCTOR (Python, deterministic)
    ├── AST-parses stubs → StubManifest
    └── Partitions into assignments
           │
    TEST SWARM (standard ×N, parallel via agent pool)
    ├── Agent A: fills tests/test_dedup.py → confirms RED
    ├── Agent B: fills tests/test_pubmed.py → confirms RED
    └── Agent C: fills tests/test_export.py → confirms RED
           │
    CONDUCTOR: verifies all RED, commits tests
           │
    IMPL SWARM (standard ×N, parallel via agent pool)
    ├── Agent A: fills src/dedup.py → runs tests → GREEN
    ├── Agent B: fills src/pubmed.py → runs tests → GREEN
    └── Agent C: fills src/export.py → runs tests → GREEN
           │
    CONDUCTOR: verifies GREEN, commits per module
```

### Decisions

- **Warm agent pool over cold `agent --print`** — eliminates 5-10s startup per step. Sessions stay warm, `/model` switches tiers, `/clear` resets context.
- **File-based I/O over terminal parsing** — agent writes output to `.conductor/steps/` files. Conductor watches filesystem. No fragile terminal output parsing.
- **Pydantic validation as determinism layer** — structured JSON contracts between steps. Retry with error message on validation failure.
- **Python AST for stub inventory** — fully deterministic, no agent needed for parsing code structure.
- **Hybrid conductor** — Python for mechanical work (git, gh, file parsing), agent pool for creative work only.
- **Local SQLite over GitHub for state** — fast polling, no API rate limits. GitHub synced as projection.
- **Test-first batch swarming** — all tests written (RED) and committed before any implementation (GREEN). Clean TDD git history + parallel execution.
- **`/clear` between steps, `/compress` within phases** — fresh context prevents pollution, compress preserves useful context when intentional.
- **`.conductor/conductor.toml` for model/timeout config** — step-level model assignment, overridable per-project. Single source of truth for conductor behavior.

### Conductor configuration: `.conductor/conductor.toml`

Model and timeout assignments per step, analogous to `.cursor/agent-models.toml` but scoped to the conductor's micro-steps.

```toml
# .conductor/conductor.toml
# Single source of truth for conductor step configuration.
# Referenced by: conductor.py

[pool]
max_sessions = 3
idle_ttl_seconds = 60       # drain idle sessions after this
default_model = "sonnet-4.5"

[models]
# Named model tiers — conductor resolves these to actual model IDs
autonomous = "opus-4.6"
standard   = "sonnet-4.5"
lightweight = "composer-1.5"

[timeouts]
# Per-step-type timeout in seconds before nudge/escalation
design    = 300
plan      = 180
architect = 300
test      = 120
implement = 180
verify    = 60
pr        = 120

[health]
poll_interval_seconds = 5    # how often to check agent health
idle_threshold_seconds = 30  # no output for this long = idle
max_nudges = 2               # nudge attempts before kill+retry
max_retries = 1              # fresh session retries before escalate

# Step-to-model mapping
# Format: step_pattern = "model_tier"
# Patterns: exact match ("1.2") or prefix match ("4.2.*")
[steps]
"1.1" = "python"         # context loading — no agent
"1.2" = "autonomous"     # design — needs codebase exploration
"1.3" = "python"         # post comment — no agent

"2.1" = "python"
"2.2" = "autonomous"     # planning — needs design understanding
"2.3" = "python"

"3.1" = "python"
"3.2" = "autonomous"     # test matrix — needs architectural reasoning
"3.3" = "autonomous"     # stub writing — needs code generation
"3.4" = "python"         # AST parsing — deterministic

"4.1" = "python"
"4.2.*" = "standard"     # test body writing — scoped, mechanical
"4.3" = "python"

"5.1" = "python"
"5.2.*" = "standard"     # implementation — scoped, test-guided
"5.3" = "python"
"5.4" = "standard"       # retry failures

"6.1" = "python"
"6.2" = "standard"       # verify fix — debugging
"6.3" = "python"

"7.1" = "python"
"7.2" = "standard"       # PR text — structured writing
"7.3" = "python"
"7.4" = "python"

# Override per step type for custom timeouts (optional)
# [steps.timeouts]
# "3.2" = 600  # architect gets extra time
```

This means changing which model handles a step is a one-line config change — no code edits. A project that wants to use cheaper models for everything can override `[models]`, and a project with particularly complex architecture can bump `"3.2" = "autonomous"` to a specific model.

**Defaults in source, overrides in TOML:**

All values above are compiled-in defaults in the conductor source code (e.g., `conductor/defaults.py`). The TOML file is optional — if missing or corrupt, the conductor regenerates it from source defaults via `just conductor --init`. Users only need to add entries that differ from defaults.

```python
# conductor/defaults.py — single source of truth for all defaults
# .conductor/conductor.toml is generated from these and overrides selectively.

from __future__ import annotations

POOL_DEFAULTS = {
    "max_sessions": 3,
    "idle_ttl_seconds": 60,
    "default_model": "sonnet-4.5",
}

MODEL_DEFAULTS = {
    "autonomous": "opus-4.6",
    "standard": "sonnet-4.5",
    "lightweight": "composer-1.5",
}

TIMEOUT_DEFAULTS = {
    "design": 300,
    "plan": 180,
    "architect": 300,
    "test": 120,
    "implement": 180,
    "verify": 60,
    "pr": 120,
}

HEALTH_DEFAULTS = {
    "poll_interval_seconds": 5,
    "idle_threshold_seconds": 30,
    "max_nudges": 2,
    "max_retries": 1,
}

# Step-to-model mapping. "python" = conductor handles directly.
STEP_DEFAULTS: dict[str, str] = {
    "1.1": "python",       "1.2": "autonomous",  "1.3": "python",
    "2.1": "python",       "2.2": "autonomous",  "2.3": "python",
    "3.1": "python",       "3.2": "autonomous",  "3.3": "autonomous",  "3.4": "python",
    "4.1": "python",       "4.2.*": "standard",  "4.3": "python",
    "5.1": "python",       "5.2.*": "standard",  "5.3": "python",       "5.4": "standard",
    "6.1": "python",       "6.2": "standard",    "6.3": "python",
    "7.1": "python",       "7.2": "standard",    "7.3": "python",       "7.4": "python",
}
```

**Config resolution order:** source defaults → `.conductor/conductor.toml` overrides → CLI flags (future).

**Regeneration:** `just conductor --init` writes a fresh TOML from source defaults. Existing TOML is backed up to `.conductor/conductor.toml.bak` before overwrite.

### Agent health monitoring and recovery

The conductor monitors each dispatched agent session and handles failure modes.

**Detection via tmux:**
```bash
# Is the process alive?
tmux display-message -t agent-0 -p '#{pane_pid}'

# When was the last output?
tmux display-message -t agent-0 -p '#{pane_activity}'

# Is the agent at its input prompt (idle)?
tmux capture-pane -t agent-0 -p -S -3
```

**State machine per dispatch:**

| State | Detection | Action |
|---|---|---|
| **active** | Last pane activity < 30s | Wait, keep polling |
| **done** | Output file exists on disk | Read, validate, proceed |
| **idle** | No activity for 30s, elapsed < timeout | Wait (model may be thinking) |
| **hung** | No activity for 30s, elapsed > timeout | Nudge, then escalate |
| **forgot** | Agent at prompt, no output file | Nudge with reminder to write file |
| **dead** | Pane PID not running | Respawn session, retry step |

**Recovery protocol (three-tier):**

1. **Nudge** — send a follow-up message to the same session reminding it to write the output file. Max 2 nudges.
2. **Retry on fresh session** — kill the session, acquire a new one, replay the same step input. Max 1 retry.
3. **Escalate** — post `## Question` on the GitHub issue, add `phase:stuck` label, move to next issue.

**Validation retry:** If the output file exists but fails Pydantic validation, the conductor sends the validation error back to the same session:
```
Your output file was invalid. Error: {validation_error}. 
Re-read the input at {input_file} and write corrected JSON to {output_file}.
```
Max 2 validation retries before fresh session retry.

---

# [Comment #2]() by [gerchowl]()

_Posted on February 27, 2026 at 12:26 AM_

## Implementation Complete

Conductor v0.1 has been built at [private-repo link]

### Architecture (12 modules, 286 tests, 95% coverage)

| Tier | Module | Description |
|---|---|---|
| 1 | `models.py` | 14 Pydantic inter-step contracts |
| 1 | `defaults.py` | Compiled-in config + TOML generation |
| 1 | `state_db.py` | SQLite schema + CRUD (issues/steps/sync) |
| 2 | `config.py` | Defaults + TOML merge + step resolution |
| 2 | `pool.py` | Warm tmux agent pool lifecycle |
| 2 | `gh_sync.py` | GitHub read/write via gh CLI |
| 3 | `dispatch.py` | Step dispatch + Pydantic validation + retry |
| 3 | `health.py` | Agent state detection + 3-tier recovery |
| 4 | `phases.py` | 7 phase handlers with swarm dispatch |
| 4 | `dag.py` | Dependency parsing + topological sort |
| 5 | `runner.py` | Main loop + Rich Live dashboard |
| 5 | `cli.py` | CLI with run/init subcommands |

### Integration

Installed in scitadel as dev dependency. Available via:
- `just conductor-init` — generate `.conductor/conductor.toml`
- `just conductor` — run the orchestrator

