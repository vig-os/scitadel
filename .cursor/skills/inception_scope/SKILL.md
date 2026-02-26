---
name: inception_scope
description: Convergent scoping — define what to build and what not to build.
disable-model-invocation: true
---

# Inception: Scope

Define what to build and what not to build through convergent scoping. This is the second phase of the inception pipeline, focusing on **what** after understanding **why**.

**Rule: Make explicit in/out decisions. Articulate MVP vs full vision.**

## When to Use

Use this skill when:
- Problem is well-understood (from [inception_explore](../inception_explore/SKILL.md))
- Need to define solution boundaries
- Need to choose between multiple approaches
- Need to size the work for planning

Skip this skill when:
- Problem isn't yet clear (go back to explore)
- Solution is already scoped (jump to [inception_architect](../inception_architect/SKILL.md))

## Precondition: RFC Problem Brief Exists

Before starting, ensure:
1. An RFC file exists in `docs/rfcs/` with Problem Statement filled
2. The problem is well-understood and approved
3. No branch required — still in pre-development phase

## Workflow Steps

### 1. Load the Problem Brief

**Read the existing RFC:**
- Locate: `docs/rfcs/RFC-XXX-YYYY-MM-DD-*.md`
- Review: Problem Statement, stakeholders, prior art, risks
- Confirm: User agrees the problem is still valid as written

### 2. Solution ideation

**Prompt:**
> "Now that we understand the problem, what are possible ways to solve it? Let's brainstorm approaches before converging."

**Generate 2-4 solution approaches:**
- **Approach 1**: Description, key idea, what makes it attractive
- **Approach 2**: Different approach, trade-offs
- **Approach 3**: Another angle (if relevant)

**For each approach, consider:**
- How does it solve the problem?
- What's the rough complexity?
- What are the main trade-offs?
- What prior art supports this?

**Present approaches to user and ask:**
> "Which approach feels most promising? Or should we combine elements?"

### 3. In/Out decisions (MVP vs Full Vision)

**Prompt:**
> "Let's define the boundaries. What's in scope for the first version (MVP), and what's future work?"

**Guide the user through scoping questions:**

#### Core functionality
- What's the **minimum** needed to solve the problem?
- What's essential vs nice-to-have?
- What can users live without initially?

#### In scope
- List features/capabilities that **will** be included in MVP
- Be specific: not "user management" but "user login with email/password"

#### Out of scope (for now)
- List features/capabilities that **won't** be in MVP but may come later
- Document why: complexity, dependencies, or diminishing returns

#### Future vision
- What's the full vision beyond MVP?
- What capabilities come in later phases?

**Document in RFC under Proposed Solution section.**

### 4. Build vs Buy assessment

**Prompt:**
> "For each major component, should we build, buy, or integrate existing tools?"

**For each component/capability:**
- **Build**: Custom implementation — when? (unique needs, tight integration)
- **Buy/Use**: Existing tool/library — when? (commodity functionality, time savings)
- **Integrate**: Combine existing pieces — when? (ecosystems exist, avoid reinvention)

**Document decision and rationale for each.**

### 5. Feasibility checks

**Prompt:**
> "Let's validate this is achievable. What constraints do we need to check?"

**Check against constraints:**

#### Technical feasibility
- Do we have the technical capability?
- Are there known blockers?
- What's the technology risk level?

#### Resource feasibility
- Skills: Do we have the needed expertise?
- Time: Rough estimate (days? weeks? months?)
- Budget: Any cost implications? (services, licenses, infrastructure)

#### Dependency feasibility
- What do we depend on? (external APIs, team deliverables, infrastructure)
- Are dependencies stable and available?
- What happens if a dependency fails?

**If NOT feasible, revisit scope or approach.**

### 6. Success criteria

**Prompt:**
> "How will we know this worked? Let's define success."

**Define measurable success criteria:**
- **User-facing success**: What can users now do that they couldn't before?
- **Metrics**: What numbers improve? (usage, performance, errors reduced)
- **Acceptance criteria**: What must be true for us to call this "done"?

**Be specific and measurable.**

### 7. Phasing (if large)

If the scope is large, break into phases:

**Prompt:**
> "This seems large. Should we break it into phases?"

**Define phases:**
- **Phase 1 (MVP)**: Core functionality, smallest useful increment
- **Phase 2**: Next set of capabilities
- **Phase 3+**: Future enhancements

**For each phase:**
- Scope: What's included
- Deliverables: What ships
- Success criteria: How we know it worked
- Dependencies: What must complete first

**Document in RFC under Phasing section.**

### 8. Complete the RFC

Fill in the remaining RFC sections:

1. **Proposed Solution**: Solution approach chosen, MVP scope, in/out decisions
2. **Alternatives Considered**: Other approaches considered and why not chosen
3. **Impact** (update):
   - Dependencies: External/internal dependencies identified
   - Risks (update): Feasibility risks, dependency risks
4. **Phasing**: If applicable, phase breakdown
5. **References** (update): Add any new research or prior art discovered

**Update RFC status**: `draft` → `proposed`

### 9. Review with user

**Present the complete RFC:**
> "Here's the full RFC with problem and proposed solution. Does this capture what we want to build?"

**Iterate** until the user approves.

### 10. Decide: Continue to architecture or stop

**Prompt:**
> "This RFC defines what to build. Should we continue to architecture design, or pause here?"

**If continue:** Proceed to [inception_architect](../inception_architect/SKILL.md)

**If pause:** Save RFC, create tracking issue if needed, hand off to human review

**If stop:** Update RFC status to `rejected`, document why

## Important Notes

- **Be decisive.** Convergent thinking requires making choices and trade-offs.
- **Document what's OUT.** Saying "no" is as important as saying "yes."
- **Use prior art.** Don't reinvent what exists unless there's a strong reason.
- **Validate feasibility.** Don't propose what's not achievable.
- **Keep it real.** MVP should be genuinely minimal and useful.

## Outputs

- Complete RFC in `docs/rfcs/` with Proposed Solution, Alternatives, Phasing
- Clear MVP scope and in/out decisions
- Build vs buy decisions for major components
- Validated feasibility

## Next Phase

After user approval, invoke [inception_architect](../inception_architect/SKILL.md) to define the system architecture.
