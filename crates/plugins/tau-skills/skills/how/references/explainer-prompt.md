# Explainer prompt template

Follow this when you write the explanation. For a complex question, the explorer findings are the reports your explorers returned, or the reports you wrote yourself angle by angle. For a simple question there are no findings: explore and explain in one pass.

---

You are writing an architectural explanation for a senior engineer. Explorers have traced different slices of the codebase and gathered findings. Synthesize their findings into one coherent, well-structured explanation.

## Original question

> {QUESTION}

## Explorer findings

{EXPLORER_FINDINGS_ALL}

## Instructions

The explorers each investigated a different angle of the same subsystem. Their findings will overlap in places and may occasionally contradict. Reconcile them. Merge overlapping descriptions, resolve contradictions by checking the code yourself, and combine the separate slices into a unified picture.

Write an explanation a senior engineer unfamiliar with this area could read and walk away with a solid mental model, understanding the architecture well enough to start working in it confidently.

Check anything you need in the code, clarify a detail, or fill a gap with `read`, `grep` and `find`. Do not edit files. The explorers did the work, so you shouldn't need to re-explore from scratch.

## Output format

Use this structure, adapted to what makes sense for the question. Not every section is needed for every question.

### Overview
1-2 paragraphs. What is this thing, what does it do, why does it exist. Someone should be able to read just this and decide whether to keep reading.

### Key concepts
The important types, services, or abstractions needed to follow the rest. Brief definitions, not exhaustive.

### How it works
The core of the explanation, and the longest section. Walk through the flow: what triggers it, what happens step by step, where data goes, what the decision points are.

Use prose, not pseudocode. Reference specific files and functions so the reader knows where to look, but don't dump large code blocks unless a snippet is essential to a point.

When the flow involves multiple components talking to each other, or data transforming through stages, include a diagram. Use Mermaid (a ```mermaid block) for structured flows (sequence diagrams, flowcharts, component graphs) or ASCII art for simpler relationships where Mermaid would be overkill. Use your judgment. A diagram should clarify, not decorate. If prose covers the flow, skip the diagram.

### Where things live
A brief file and directory map. Just the ones someone would need to start working here.

### Gotchas
Non-obvious things, surprising behavior, historical context, pitfalls. Skip this section if there's nothing worth calling out.

## Communication style

- Use concrete language, not abstractions about abstractions.
- Say "`SessionStore.load` calls `Migrator.apply` before the first query", not "the store delegates to the migrator".
- When something is complex, explain why it's complex. Don't just describe the complexity.
- When something is simple, don't pad it out.
- If there's a helpful analogy, use it. If there isn't, don't force one.
- If the explorers flagged open questions or gaps, acknowledge them rather than hiding them.
