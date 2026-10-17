---
name: principle-subtract-before-you-add
description: "Load when sequencing an addition, refactor, or rewrite. Remove dead code, redundant validators, and stub references first, commit that removal on its own, then build on the simpler base."
---

# Subtract Before You Add

When you evolve a system, remove complexity first, then build. Default to subtraction.

**Why:** An addition built on dead code has to work around it, and the dead code then looks used. Removing first shrinks what the addition must fit into, and often shows the addition is smaller than planned. A removal on its own is also easy to review, because it should change no behavior.

Make simplification a continual investment. Leave the design slightly simpler and more capable, behind the same or a smaller interface than you found it.

**Pattern:**
- Sequence removal before construction. Land the removal as its own `vcs_commit` (for example `refactor: remove unused retry wrapper`) before the commit that adds anything. Run the tests on the removal commit, so it is proven to change nothing.
- Cut before you polish. Get to the minimum before you invest in quality.
- Design for observed usage, not speculative edge cases.
- Write no speculative validators, parsers or guards beyond what the spec demands.
- Simplify prompts and instructions too: remove redundant instructions and oversized templates.
- When a reference (a doc, a re-export, a stub module) has no novel content, delete it instead of leaving a stub.

You skipped this when the feature commit also deletes code, so a reviewer cannot tell which removed lines were dead and which were behavior.

Distinct from `principle-laziness-protocol`, which keeps each change small. This principle sets the order: removal first, then the addition. `principle-foundational-thinking` puts scaffolding next, after the subtraction.
