---
name: inception_plan
description: Decomposition — turn scoped design into actionable GitHub issues.
disable-model-invocation: true
---

# Inception: Plan

Decompose a scoped solution into actionable GitHub issues. This is the fourth and final phase of the inception pipeline, creating the handoff to development.

**Rule: Create traceable work items. Link everything. Make dependencies explicit.**

## When to Use

Use this skill when:
- Design is complete (from [inception_architect](../inception_architect/SKILL.md))
- Ready to create work items for development
- Need to organize work into issues and milestones

Skip this skill when:
- Design isn't finalized
- Solution is trivial (single issue sufficient, use [issue_create](../issue_create/SKILL.md))

## Precondition: Design Document Exists

Before starting, ensure:
1. RFC file exists with Proposed Solution
2. Design document exists with architecture defined
3. No branch required — still in pre-development phase

## Workflow Steps

### 1. Load RFC and Design

**Read existing artifacts:**
- RFC: `docs/rfcs/RFC-XXX-*.md` — scope, phasing, requirements
- Design: `docs/designs/DES-XXX-*.md` — components, architecture
- Extract: Work to be done, dependencies, phases

### 2. Decompose into work items

**Break solution into independent deliverables:**

#### Identify work streams
- What are the major pieces? (e.g., "API implementation", "UI components", "data migration")
- Can they be worked on independently?
- What's the dependency order?

#### Define issues for each work stream
For each piece, create an issue with:
- **Title**: `[TYPE] Short description`
- **Description**: What needs to be done
- **Acceptance criteria**: How do we know it's done
- **References**: Link to RFC and Design docs
- **Labels**: `feature`, `effort:small/medium/large`, `area:*`

**Guideline for sizing:**
- **Small** (effort:small): 1-3 days, clear scope
- **Medium** (effort:medium): 1-2 weeks, some complexity
- **Large** (effort:large): 2+ weeks, needs breakdown into sub-issues

#### Create parent issue
If the solution has multiple parts, create a parent issue:
- **Title**: `[EPIC] <Solution name>`
- **Description**: Overview, link to RFC and Design
- **Task list**: Links to all sub-issues
- **Labels**: `epic`, area label

### 3. Identify spikes

**Find unknowns that need proof-of-concept:**

**Prompt:**
> "Are there any technical unknowns that need investigation before we can implement confidently?"

**For each unknown:**
- What's the question?
- Why is it a risk?
- What would a spike prove/disprove?

**Create spike issues:**
- **Title**: `[SPIKE] <Question to answer>`
- **Description**: What we're investigating, why, what success looks like
- **Acceptance criteria**: Findings documented, recommendation made
- **Time-box**: 1-3 days max
- **Labels**: `spike`, `effort:small`

### 4. Map dependencies

**Identify ordering constraints:**

#### Technical dependencies
- Issue A must complete before Issue B can start
- Issue C blocks Issue D

#### Resource dependencies
- Issues that need the same person/skill
- Issues that compete for infrastructure

**Document dependencies:**
- In GitHub: Use "blocked by" relationships
- In parent issue: Note dependency order in task list

### 5. Assign to milestones

**If phasing exists (from RFC), map issues to milestones:**

#### Create milestones (if needed)

```bash
gh api repos/{owner}/{repo}/milestones \
  -f title="Phase 1: MVP" \
  -f description="<scope from RFC>" \
  -f due_on="<target-date>"
```

#### Assign issues to milestones

```bash
gh issue edit <issue-number> --milestone "<milestone-name>"
```

**Phase 1 (MVP)** gets earliest milestone, Phase 2+ gets future milestones.

### 6. Apply effort estimation

**Size each issue with effort labels:**

**Prompt the user:**
> "For issue <title>, is this small (1-3 days), medium (1-2 weeks), or large (2+ weeks)?"

**Apply labels:**

```bash
gh issue edit <issue-number> --add-label "effort:small"
gh issue edit <issue-number> --add-label "effort:medium"
gh issue edit <issue-number> --add-label "effort:large"
```

**If large:** Suggest breaking it into smaller issues.

### 7. Create issues on GitHub

**For each issue defined:**

#### Parent/Epic issue

```bash
gh issue create \
  --title "[EPIC] <title>" \
  --body "<body-with-links-to-rfc-and-design>" \
  --label "epic" \
  --label "area:<domain>"
```

#### Sub-issues

```bash
gh issue create \
  --title "[FEATURE] <title>" \
  --body "<body-with-acceptance-criteria>" \
  --label "feature" \
  --label "effort:<size>" \
  --label "area:<domain>"
```

**Link sub-issues to parent:**
- In parent issue body, add task list: `- [ ] #<sub-issue-number>`
- GitHub will auto-track completion

#### Spike issues

```bash
gh issue create \
  --title "[SPIKE] <question>" \
  --body "<investigation-scope>" \
  --label "spike" \
  --label "effort:small"
```

### 8. Link RFC and Design to issues

**Update RFC and Design docs to reference issues:**

#### In RFC
Add section:

```markdown
## Implementation Tracking

- Epic: #<parent-issue>
- Milestone: <milestone-name>
```

#### In Design doc
Add section:

```markdown
## Implementation Issues

- #<issue-1> — <component-name>
- #<issue-2> — <component-name>
...
```

**Commit and push RFC and Design updates.**

### 9. Review with user

**Present the issue structure:**
> "Here's the issue breakdown. Does this capture all the work? Are the dependencies clear?"

**Show:**
- Parent issue URL
- List of sub-issues
- Milestone assignments
- Dependency graph (if complex)

**Iterate** if needed.

### 10. Hand off to development

**Summarize handoff:**
> "Inception complete. The work is now captured in GitHub issues. The first issue to tackle is #<issue>, which has no blockers."

**Next steps for development:**
- Issues are ready for [issue_claim](../issue_claim/SKILL.md)
- Each issue will go through [design_brainstorm](../design_brainstorm/SKILL.md) → [code_execute](../code_execute/SKILL.md) workflow
- Spikes feed findings back to RFC/Design docs

## Important Notes

- **Every issue links back.** RFC and Design must be referenced in every issue.
- **Make dependencies explicit.** Use GitHub's blocking relationships.
- **Size realistically.** If it's "large", break it down further.
- **Spikes are time-boxed.** No open-ended investigation.
- **Milestones are optional.** Use them for large projects, skip for small ones.

## Outputs

- Parent issue (epic) on GitHub
- Sub-issues for each work stream
- Spike issues for unknowns
- Milestone assignments (if phased)
- Effort labels applied
- RFC and Design updated with issue links

## Handoff Complete

The inception pipeline ends here. The work now follows the normal development workflow:
- [issue_claim](../issue_claim/SKILL.md) to start work
- [design_brainstorm](../design_brainstorm/SKILL.md) for per-issue design
- [code_execute](../code_execute/SKILL.md) for implementation
