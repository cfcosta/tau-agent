---
name: principle-model-the-domain
description: "Load when writing stateful logic, or when code branches a lot or repeats a shape assumption across files. Encode the domain in a structure (state machine, typed model, table, reducer, module boundary) instead of scattered conditionals."
---

# Model the Domain

Encode the real domain in a data structure instead of scattering it across conditionals. A structure that matches the domain makes invalid states unrepresentable and deletes branches.

**Why:** Choosing the structure at write time is cheap. Recovering it later reads as a refactor and gets deferred, while every new feature adds another branch to the scattered version.

**Reach for:**
- A state machine instead of scattered booleans, phases or lifecycle checks. In a typed language, a tagged union (sum type) whose cases carry each state's data: a job that is queued, running with its process id, or done with its exit status. Handle it exhaustively, with no catch-all branch, so a new state breaks the build everywhere that must decide about it.
- A typed model instead of loose parameters or repeated shape assumptions: a record type in place of a tuple, a string-keyed map of untyped values, or five arguments that always travel together. A distinct type (a duration in milliseconds, not a bare integer) when a primitive carries a meaning or a rule.
- A map, registry, lookup table or tagged union instead of branching spread across files. One table that lists every kind with its properties replaces the same branch on the kind copied into five modules.
- A reducer or command/event model instead of ad hoc state mutations: one `apply(state, event)` function that returns the next state puts every transition in one place.
- A module organized around one body of domain knowledge instead of a sequence such as load, validate, transform and save. Execution order is not ownership.
- A small module boundary that gathers repeated behavior, ownership or invariants.
- A queue, cache, index, graph, tree or normalized collection where the access pattern calls for it.
- Any other structure that fits. When none fits, work out what the code must never allow and how the data gets read, then find the structure that encodes exactly that.

Do not force an abstraction. Prefer boring code when the current shape is already clear, local and unlikely to grow. Be skeptical of an abstraction that adds indirection without removing branches, duplicated rules, invalid states or lifecycle risk.

You skipped this when a new feature grows an existing if/else chain by one more branch, or adds a second boolean that must stay in sync with the first. Temporal decomposition is another sign: phase-named modules that repeat the same domain rules across steps.

Distinct from `principle-type-system-discipline`, which makes the compiler hold a structure in place. This principle picks the structure. Distinct from `principle-foundational-thinking`, which says to choose data before logic; this one says what to choose.
