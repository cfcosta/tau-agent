---
name: principle-foundational-thinking
description: "Load before writing logic: choosing core types and data structures, ordering scaffold against feature work, or asking what concurrent actors (threads, processes, parallel sub-agents) share. Get the data structures right so the code that follows is obvious."
---

# Foundational Thinking

Structural decisions protect option value. Code-level decisions protect simplicity.

**Why:** Data shapes are the hardest thing to change later. Every function written against a wrong shape must be rewritten when the shape changes. Logic over the right shape is short and often needs no branches.

**Data structures first.** Get the data shape right before you write logic. Define the core types early, trace every access pattern (who reads, who writes, how often, in what order), and choose structures that match the dominant paths.

**At code level, DRY the structure, not every line.** Types and data models should converge on one definition. Three similar statements still beat a premature abstraction. Prefer explicit code over clever code. Test behavior and edge cases, not line counts.

**Concurrency corollary.** Before two actors share state, ask "what happens if another actor modifies this concurrently?" If the answer is not "nothing", isolate them. The actors are threads, processes, async tasks, and several writers on one database or file. In tau they are also parallel sub-agents working in one repository: two that edit the same file land as conflicts on their caller. Give each sub-agent its own files or directories, or run the clashing tasks one after another.

**Scaffold first.** If something helps every later phase, do it first. Ask "does every later phase benefit from this existing?" Test infrastructure, lint setup, shared types and a verification script are scaffold. Order for option value: setup before features, a failing test before the fix.

**Small, single-purpose commits.** Land the scaffold as its own `vcs_commit`, then each feature step as its own. A reviewer reads the foundation once and checks each step against it.

Each increment should land a coherent abstraction or deepen one that exists. Do not spread a new capability across callers as special-case coordination.

Subtraction comes before scaffolding: remove dead code first, then lay foundations (load the `principle-subtract-before-you-add` skill).

Distinct from `principle-model-the-domain`, which picks the structure that encodes the domain's rules. This principle sets the order: data before logic, scaffold before features. `principle-experience-first` sets the target; this one governs the sequence.
