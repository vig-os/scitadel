---
name: inception_explore
description: Divergent exploration — understand the problem space before jumping to solutions.
disable-model-invocation: true
---

# Inception: Explore

Understand the problem space through divergent exploration. This is the first phase of the inception pipeline, focusing on **why** before **what** or **how**.

**Rule: No solutions yet. Only questions, research, and problem articulation.**

## When to Use

Use this skill when:
- Starting with a vague idea or signal ("we should probably...")
- Received feedback or feature request that needs unpacking
- Research finding suggests an opportunity
- Problem exists but isn't well-understood yet

Skip this skill when:
- Problem is already well-articulated (jump to [inception_scope](../inception_scope/SKILL.md))
- It's a small, obvious fix (use existing issue workflow)

## Precondition: No Branch Required

Unlike development skills, inception happens **before** issues and branches exist. You're working from the main/dev branch or no repo at all.

## Workflow Steps

### 1. Capture the signal

**Prompt the user:**
> "Let's explore this idea. Can you describe the signal that brought this up? What made you think we need to look at this?"

**Record:**
- Source: Where did this come from? (user feedback, team discussion, metrics, research)
- Initial framing: How was it initially described?
- Urgency: Is this blocking anyone? Time-sensitive?

### 2. Problem framing

**Guide the user through problem articulation with these questions** (ask one at a time, don't overwhelm):

#### What's actually wrong?
- What pain point exists today?
- What's the current workaround?
- What happens if we do nothing?

#### Who's affected?
- Who experiences this problem directly?
- Who else is indirectly impacted?
- What's the impact severity? (minor annoyance → critical blocker)

#### When does it happen?
- Is it always present or situational?
- What triggers it?
- Has it gotten worse over time?

#### Why does it matter?
- What's the business/user impact?
- How does this align with project goals?
- What would success look like?

**Document the answers** in the RFC draft as you go.

### 3. Stakeholder mapping

**Identify who cares about this:**

**Prompt:**
> "Who should have input on this? Let's map the stakeholders."

**Map:**
- **Deciders**: Who approves/rejects this?
- **Contributors**: Who will build it?
- **Users**: Who will use it?
- **Affected parties**: Who will be impacted by it?

**Note their concerns, constraints, and success criteria.**

### 4. Prior art and research

**Prompt:**
> "Let's look at what already exists. Has anyone solved this before?"

**Research (with user input and web search):**
- Open-source solutions: What tools/libraries exist?
- Competitors: How do others solve this?
- Standards: Are there established patterns or specs?
- Academic research: Any relevant papers or studies?

**Document findings:**
- What exists
- How it solves (or doesn't solve) our problem
- What we can learn/borrow
- What gaps remain

### 5. Assumptions surfacing

**Prompt:**
> "What are we assuming that might not be true?"

**Challenge assumptions:**
- About the problem: Are we sure this is the real problem?
- About users: Are we assuming needs without validating?
- About solutions: Are we prematurely converging on an approach?
- About feasibility: Are we assuming technical constraints that may not exist?

**Document assumptions** and flag high-risk ones for validation.

### 6. Risk identification

**Prompt:**
> "What could go wrong? Let's identify risks early."

**Explore risks:**
- **Technical risks**: Hard to implement? Scalability concerns?
- **Regulatory risks**: Legal, compliance, security issues?
- **Resource risks**: Skills, time, budget constraints?
- **Dependency risks**: Reliant on external factors?

**Document each risk with severity and mitigation ideas.**

### 7. Draft the Problem Brief

Synthesize all findings into the early sections of an RFC document:

1. Create RFC file: `docs/rfcs/RFC-XXX-YYYY-MM-DD-<kebab-case-title>.md`
2. Use the [RFC template](../../../docs/templates/RFC.md)
3. Fill in:
   - Problem Statement (from step 2)
   - Impact section (stakeholders from step 3)
   - References (prior art from step 4)
   - Open Questions (assumptions and risks from steps 5-6)

**Leave Proposed Solution and Alternatives sections empty** — that's for the next phase.

### 8. Review with user

**Present the draft RFC:**
> "Here's what I've captured so far. Does this accurately represent the problem? What's missing?"

**Iterate** until the user confirms the problem is well-understood.

### 9. Decide: Continue or stop

**Prompt:**
> "Based on this exploration, should we continue to scoping? Or is this not worth pursuing?"

**If continue:** Proceed to [inception_scope](../inception_scope/SKILL.md)

**If stop:** Document why in the RFC (status: rejected) and close gracefully.

## Important Notes

- **Stay in problem space.** Push back if the user jumps to solutions prematurely.
- **One question at a time.** Don't overwhelm with a wall of questions.
- **Be skeptical.** Challenge assumptions and dig deeper when answers are vague.
- **No code yet.** This is purely exploratory. No implementation, no branches.
- **Document as you go.** The RFC is the living record of the exploration.

## Outputs

- RFC file in `docs/rfcs/` with Problem Statement, Impact, and References sections filled
- Shared understanding of the problem
- Decision to continue to scope phase or stop

## Next Phase

After user approval, invoke [inception_scope](../inception_scope/SKILL.md) to define what to build.
