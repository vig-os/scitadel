# Inception Skills

Pre-development product-thinking skills that bridge the gap between "I have an idea" and "I have actionable GitHub issues."

## Overview

The inception skill family covers four phases of product thinking:

```
Signal → inception_explore → inception_scope → inception_architect → inception_plan → [development workflow]
         (diverge)           (converge)       (evaluate)            (decompose)
```

Each phase produces durable artifacts (RFCs, Design documents, GitHub issues) that serve as the single source of truth for what's being built and why.

## When to Use

Use inception skills when:
- Starting with a vague idea or signal that needs exploration
- Problem isn't yet well-understood
- Solution needs formal architecture review
- Work requires decomposition into multiple issues/milestones

Skip inception skills when:
- Problem and solution are already clear (use [issue_create](../issue_create/) and [design_brainstorm](../design_brainstorm/) directly)
- It's a small fix or enhancement (single issue sufficient)
- You're extending existing patterns (use normal dev workflow)

## Phases

### 1. inception_explore (Divergent)

**Purpose:** Understand the problem space before jumping to solutions.

**Activities:**
- Problem framing
- Stakeholder mapping
- Prior art research
- Assumptions surfacing
- Risk identification

**Output:** RFC Problem Brief (`docs/rfcs/RFC-NNN-YYYY-MM-DD-title.md`)

**Interaction:** Guided/interactive — agent asks probing questions, pushes back on premature solutions.

**Skip when:** Problem is already well-articulated.

---

### 2. inception_scope (Convergent)

**Purpose:** Define what to build and what not to build.

**Activities:**
- Solution ideation
- In/out decisions (MVP vs full vision)
- Build vs buy assessment
- Feasibility checks
- Success criteria definition
- Phasing (if large)

**Output:** Complete RFC with Proposed Solution

**Interaction:** Draft & review for clear ideas; guided/interactive for ambiguous ones.

**Skip when:** Solution is already scoped.

---

### 3. inception_architect (Evaluative)

**Purpose:** Define system architecture and validate against established patterns.

**Activities:**
- Pattern discovery from certified repos + web search
- Pattern comparison matrix
- Component topology (mermaid diagrams)
- Technology stack evaluation
- Blind spot check (observability, security, scalability, etc.)
- Deviation justification

**Certified architecture references** (embedded in skill):
- ByteByteGoHq/system-design-101
- donnemartin/system-design-primer
- karanpratapsingh/system-design
- binhnguyennus/awesome-scalability
- mehdihadeli/awesome-software-architecture

**Output:** Design document (`docs/designs/DES-NNN-YYYY-MM-DD-title.md`)

**Interaction:** Research-driven, presents comparisons.

**Skip when:** Solution is trivial or architecture is already established.

---

### 4. inception_plan (Analytical)

**Purpose:** Decompose scoped design into actionable GitHub issues.

**Activities:**
- Work breakdown into independent deliverables
- Spike identification (proof-of-concept for unknowns)
- Dependency mapping
- Milestone assignment
- Effort estimation (effort:small/medium/large labels)
- Issue creation (parent + sub-issues)

**Output:** GitHub parent issue with linked sub-issues

**Interaction:** Draft & review.

**Skip when:** Solution fits in a single issue.

---

## Scaling by Idea Size

| Idea Size | explore | scope | architect | plan |
|-----------|---------|-------|-----------|------|
| **Small** (one issue) | Quick/skip | Quick/skip | Skip | 1 issue |
| **Medium** (few issues) | Guided | Draft & review | Light comparison | Parent + sub-issues |
| **Large** (multi-milestone) | Deep guided | Interactive | Full pattern eval | Parent + sub + milestones |

Agent detects size from conversation and suggests skipping phases when appropriate.

## Key Properties

- **No branch required** — inception happens before issues exist; work from main/dev
- **Phases are skippable** — agent suggests skipping for small ideas
- **Artifacts are durable** — RFCs and designs in repo, issues on GitHub, version-controlled alongside code
- **Spikes loop back** — unknowns spawn spike issues that feed findings back to RFC/Design docs
- **Handoff is human "go"** — no formal approval gates, just human review between phases

## Document Templates

Located in `docs/templates/`:
- **RFC.md** — Problem Statement, Proposed Solution, Alternatives, Impact, Phasing
- **DESIGN.md** — Architecture, Components, Data Flow, Technology Stack, Testing

## Handoff to Development

After `inception_plan` creates GitHub issues:
1. Use [issue_claim](../issue_claim/) to start work on an issue
2. Each issue goes through [design_brainstorm](../design_brainstorm/) → [code_execute](../code_execute/) workflow
3. Spikes feed findings back to RFC/Design docs

## Example Flow

### Small idea (skip most phases)

```
User: "Add a --debug flag to the install script"
→ inception_scope (quick in/out) → create single issue → [dev workflow]
```

### Medium idea

```
User: "Add support for custom post-install hooks"
→ inception_explore (problem framing)
→ inception_scope (scope hook types, MVP vs full)
→ inception_plan (parent issue + 3 sub-issues)
→ [dev workflow]
```

### Large idea

```
User: "Add multi-tenancy to the system"
→ inception_explore (deep problem understanding, stakeholder mapping)
→ inception_scope (phasing, MVP vs full vision)
→ inception_architect (pattern comparison, blind spot check)
→ inception_plan (parent issue + 15 sub-issues across 3 milestones)
→ [dev workflow]
```

## References

- [RFC template](../../templates/RFC.md)
- [DESIGN template](../../templates/DESIGN.md)
- [Keep a Changelog](https://keepachangelog.com/) — format for CHANGELOG.md entries
- [Single Source of Truth rule](../../../.cursor/rules/single-source-of-truth.mdc)
