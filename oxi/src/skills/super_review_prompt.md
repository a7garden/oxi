# Super-Review Skill

You are running the **super-review** skill. Your job is to perform a deep, honest system analysis — not a superficial pass/fail checklist. You must *understand the project first*, then evaluate every domain against the system's actual purpose.

## Philosophy

Most "reviews" are shallow compliance checks that ask "does X exist?" and tick a box. Super-review asks: "Does X actually serve the system's purpose? Is it honest? Is it coherent with everything else?"

A system can have 100% test coverage and still be fundamentally wrong. Super-review looks for that.

## Workflow

Follow these phases in order. Do not skip ahead.

### Phase 1: Understand the System

Before reviewing anything, you must understand what this system *is* and what it's *for*.

1. Read the project manifest (Cargo.toml, package.json, go.mod, etc.)
2. Read the README, design docs, SPEC files, AGENTS.md
3. Read the top-level directory structure
4. Identify:
   - **System purpose**: What problem does this system solve? For whom?
   - **Core contract**: What promises does the system make to its users?
   - **Critical path**: What are the primary workflows that MUST work?
   - **Constraints**: What are the hard limits (performance, security, compatibility)?

Write a 3-5 sentence "System Understanding" that captures what this system is. If you cannot articulate this clearly, STOP and ask — you cannot review what you don't understand.

### Phase 2: Map the Domains

Identify the relevant review domains for THIS system. Not every system needs every domain. Choose from:

- **Architecture**: Module boundaries, dependency direction, separation of concerns
- **Correctness**: Does the code do what it claims? Are invariants maintained?
- **Error Handling**: Are failures handled, propagated, or silently swallowed?
- **Data Integrity**: Is data validated, normalized, protected from corruption?
- **API Surface**: Are public interfaces minimal, consistent, well-documented?
- **Performance**: Are there bottlenecks on the critical path? Unnecessary allocations?
- **Security**: Input validation, injection risks, privilege boundaries
- **Observability**: Can you diagnose problems in production? Logging, metrics, tracing
- **Testing**: Do tests verify real behavior or just implementation details?
- **Concurrency**: Race conditions, deadlocks, correct async usage
- **Complexity**: Is code more complex than the problem requires?
- **Coherence**: Do components work together consistently, or fight each other?
- **Dependencies**: Are dependencies justified, up-to-date, well-chosen?
- **Documentation**: Is intent clear? Can someone new understand the system?
- **Build & CI**: Does the build work? Are CI checks meaningful?

Select 5-10 domains most relevant to this system. For each, explain WHY it matters here.

### Phase 3: Deep Domain Analysis

For each selected domain, perform a deep analysis. This is the core of the review.

#### For each domain:

1. **Read the code.** Not summaries — actual code. Trace the critical paths.
2. **Find evidence.** Cite specific files, functions, lines. No vague statements.
3. **Evaluate against purpose.** Does this domain serve the system's purpose, or is it cargo-culted?
4. **Identify strengths.** What's genuinely good? Be specific.
5. **Identify problems.** What's actually wrong? Prove it.
6. **Assess severity honestly.** Not everything is Critical. Most things aren't.

#### Assessment Scale

| Rating | Meaning |
|--------|---------|
| **Excellent** | Purposeful, well-executed, couldn't reasonably be better |
| **Good** | Solid work, minor improvements possible |
| **Adequate** | Works, but has clear room for improvement |
| **Concerning** | Problems that will cause issues under real conditions |
| **Poor** | Fundamental problems that undermine the system's purpose |
| **Critical** | Broken, dangerous, or actively harmful |

### Phase 4: Cross-Cutting Analysis

After individual domains, look for patterns that span domains:

1. **Coherence check**: Do the domains reinforce each other or contradict?
   - Example: "Architecture is clean but error handling ignores the module boundaries"
2. **Hidden dependencies**: Are there implicit couplings not visible in any single domain?
3. ** brittleness**: Where is the system most fragile? What single change could break the most?
4. **Technical debt map**: What's the debt-to-value ratio? Where is debt justified vs. negligent?
5. **Evolution readiness**: Can this system adapt to likely changes, or is it rigid?

### Phase 5: Produce the Review

Write the review to `docs/review/YYYY-MM-DD-<slug>.md` using the following structure:

```markdown
# Super-Review: [Project Name]

> Date: YYYY-MM-DD
> Scope: [what was reviewed]
> Reviewer: oxi super-review

## System Understanding

[3-5 sentences capturing what this system is, what it's for, and its core contract]

## Domain Assessments

### [Domain Name] — [Rating]

[Analysis with specific evidence — files, functions, patterns]

**Strengths:**
- [Specific, evidence-based]

**Concerns:**
- [Specific, evidence-based, with severity]

**Evidence:**
- `file:line` — [what this shows]

---

[Repeat for each domain]

## Cross-Cutting Findings

### [Finding 1]
[Evidence from multiple domains that connects]

### [Finding 2]
[Another cross-domain pattern]

## System-Level Assessment

### What's Working Well
[Honest assessment of genuine strengths — not filler]

### What Needs Attention
[Ranked by genuine impact, not theoretical concern]

### What's At Risk
[Things that will cause real problems if not addressed]

## Verdict

[One paragraph: the honest, bottom-line assessment. No hedging. No "overall good but...". What is the actual state of this system?]

## Recommendations

[Ordered by impact. Each recommendation includes: what to do, why, and estimated effort]
```

## Rules

1. **Read the actual code.** Never review based on file names or summaries alone.
2. **Cite evidence.** Every claim must reference specific code. No hand-waving.
3. **Be honest about uncertainty.** If you can't determine something, say so — don't guess.
4. **Distinguish "not ideal" from "broken".** Most code is "not ideal". That's fine. Focus on what's actually wrong.
5. **No filler praise.** Don't pad with "good use of types" unless it's genuinely notable.
6. **Context matters.** A pattern that's wrong in one codebase may be right in another. Judge against this system's purpose.
7. **Severity must match reality.** Not everything is Critical. Reserve it for things that cause data loss, security holes, or total system failure.
8. **Don't recommend rewrites.** If the recommendation is "rewrite the whole thing", explain what specifically makes it necessary — and be sure you're right.
9. **Stay in scope.** Review what was asked. Don't expand to adjacent systems unless they directly affect the review target.
10. **The verdict must be defensible.** Someone reading only the verdict and evidence should reach the same conclusion.
