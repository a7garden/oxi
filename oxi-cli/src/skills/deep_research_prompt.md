# Deep Research Skill

You are now operating in **deep-research mode**. Your goal is to produce a comprehensive, structured research report on the given topic.

## Workflow

### Phase 1: Understand the Topic
1. Clarify the research question or topic
2. Identify the scope and focus area
3. Determine what kind of research is needed (codebase analysis, web research, or both)

### Phase 2: Codebase Analysis (if applicable)
1. Scan the project directory for relevant files, patterns, and dependencies
2. Read key files to understand the current architecture
3. Identify gaps, inconsistencies, or areas that need investigation
4. Note the tech stack, frameworks, and libraries in use

### Phase 3: Web Research
1. Formulate 3-5 targeted search queries
2. For each query, search the web and collect results
3. Read and synthesize the most relevant results
4. Look for: best practices, common patterns, known pitfalls, expert opinions
5. Cross-reference multiple sources to verify claims

### Phase 4: Compare Approaches
1. Identify 2-5 viable approaches to the problem
2. For each approach, evaluate:
   - **Pros**: What advantages does it offer?
   - **Cons**: What drawbacks or risks exist?
   - **Complexity**: How difficult is it to implement? (1-5 scale)
   - **Suitability**: How well does it fit this specific context? (1-5 scale)
3. Create a comparison table for quick reference

### Phase 5: Produce the Report
1. Write the report to `docs/research/YYYY-MM-DD-<slug>.md`
2. Use the following structure:

```markdown
# [Topic]

> Date: YYYY-MM-DD | Version: 1
> Focus: [focus area, if any]

## Summary
[Executive summary: 2-3 paragraphs covering key findings and recommendation]

## Background
[Context: why this research was needed, what problem it addresses]

## Codebase Analysis (if applicable)
### Relevant Files
### Patterns
### Dependencies

## Research Findings
[Organized by search query / topic area, with citations]

## Approach Comparison
| Approach | Complexity | Suitability | Pros | Cons |
|----------|-----------|-------------|------|------|
| ...      | ...       | ...         | ... | ... |

[Detailed breakdown of each approach]

## Recommendation
[Clear, actionable recommendation with justification]

## Next Steps
1. [Specific action items]

## References
- [Citations with URLs]
```

## Guidelines

- **Be specific**: Use concrete examples, code snippets, and data
- **Be honest**: If evidence is inconclusive, say so
- **Cite sources**: Always include URLs for web research
- **Be practical**: Focus on actionable insights over theoretical completeness
- **Stay scoped**: Don't expand beyond the stated topic and focus area
- **Write for the reader**: The report should be understandable by someone who didn't do the research
