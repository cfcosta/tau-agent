---
name: principle-redesign-from-first-principles
description: "Load when integrating a new requirement into an existing design. Redesign as if the requirement had been a foundational assumption from the start instead of bolting it on, then deliver the redesign as a stack of small commits."
---

# Redesign From First Principles

When you integrate a change, do not bolt it onto the existing design. Redesign as if the requirement had been a foundational assumption from the start.

**Why:** A bolted-on requirement leaves a design that describes the old world plus a patch. The next reader must learn both, and the next requirement bolts onto the patch. A redesign keeps option value: the code reads as if it was always meant to do this.

**Pattern:**
- Read every affected file and understand the current design before you change it. `grep` for every name the change touches.
- Ask: "if we were writing this from scratch with this new requirement, what would we build?" Write the answer down in a few sentences before you edit.
- Compare that answer with what exists. The gap is the redesign. If there is no gap, the design already fits and you can make the change directly.
- Propagate the change through every reference: types, tests, docs, examples, rationale comments, and the repository's instructions where they describe the old design.
- Think about the whole redesign at once, then deliver it as a stack of commits. Each `vcs_commit` is one step a reviewer can follow, and each leaves the build and tests passing where that is practical (load the `principle-sequence-verifiable-units` skill).
- When the code has no external users (an application, an internal module, a repository nothing else depends on), keep no compatibility shims for the old shape. Change the callers and delete the old path (load the `principle-migrate-callers-then-delete-legacy-apis` skill). Keep a compatibility layer only for consumers you cannot change, such as a published API or data already stored on users' machines, and say which consumer it protects.

You skipped this when the new requirement appears as an `if new_mode` branch in code that otherwise ignores it, or as an optional field that only one path reads.

Distinct from `principle-attack-the-premise`, which questions a fact the current design assumes. This principle rebuilds the design around a new requirement. Distinct from `principle-outcome-oriented-execution`, which governs how you move through the intermediate states of a large rewrite.
