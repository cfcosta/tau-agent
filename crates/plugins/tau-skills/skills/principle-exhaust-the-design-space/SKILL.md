---
name: principle-exhaust-the-design-space
description: "Load when facing a novel UI interaction, product-feel decision, or architectural decision with no precedent in the codebase, or a one-way door. Build 2-3 structurally different prototypes and compare them side by side before committing."
---

# Exhaust the Design Space

When a decision is new, build two or three competing prototypes and compare them side by side before you commit to one.

**Why:** The first design that comes to mind is a sample of one. A structurally different second design either beats it or shows why the first was right.

**When it applies.** Only to real novelty:
- The codebase has no precedent for this kind of interaction, component or architecture, so there is no pattern to follow.
- The decision is a one-way door: hard to reverse once code, data or users depend on it, such as a storage format, a public API, a protocol, or a core data model.

It also applies to product design decisions where the experience depends on feel, not logic: layout, density, motion, interaction timing. Reasoning does not settle those. Using them does.

If the codebase already does this kind of thing, follow its precedent and skip this principle. Three versions of routine work triple it for nothing.

**Pattern:**
- Write the decision down first: the question the prototypes must answer and a short rubric (what must be true, what you are trading off). Write it before any candidate exists, so the candidates cannot shape the rubric.
- Make the candidates structurally different. Whole-shape alternatives (two ownership models, two data models, two layouts), not point variations inside one shape.
- Keep each prototype throwaway: no tests, no polish.
- On the main chat, commit, then `spawn` two or three sub-agents with `land: false`, each building one candidate from its own brief and lens. Their commits stay beside yours and never land, so they may work on the real files. End the turn; each reports its changes when it finishes. Load the `arena` skill for the full procedure, including a fresh judge.
- On a chat or a sub-agent, where `spawn` is refused, sketch the variants yourself, one after another, each under its own scratch path.
- Compare side by side on the canvas: a `canvas_write` table of candidates against the rubric, plus a `canvas_show` screenshot of each visual one.
- Pick one. Take its files with `vcs_restore {paths, from: <change>}`, graft what is worth keeping from the others by hand, and commit. Spawned losers never land. Prototypes you wrote yourself, delete in your own commit. When two feel tied, prefer the cleaner boundary or the smaller API (load the `principle-laziness-protocol` skill).
- If the choice belongs to the person (product direction, a tradeoff between their preferences), put the comparison on the canvas and `ask`, with your recommendation first.

You skipped this when you cannot say what the second-best design was and why it lost.

Distinct from `principle-experience-first`, which says what to optimize for. This principle makes you look at more than one way to get there. The `architect` skill applies it to code structure ("design it twice"), and the `playbook-prototype` skill is its cheap form for one decision.
