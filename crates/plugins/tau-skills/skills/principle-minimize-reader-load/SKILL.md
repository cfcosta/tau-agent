---
name: principle-minimize-reader-load
description: "Load when reviewing or shaping code that is hard to trace. Count the layers between a question and its answer and the hidden state a reader must hold; collapse one-caller wrappers, shrink mutable scope and visibility."
---

# Minimize Reader Load

Maintainability is the work a reader must do to understand code. Track two axes: **layers to trace** (indirections between the question and the answer) and **state to hold** (hidden or mutable context the reader must keep in their head). Lines of code, cyclomatic complexity and "clean architecture" are proxies. Reader load is what matters.

The two axes are independent. A flat file with 50 globals can be as hard to reason about as a six-layer adapter stack. Guard both.

**Why:** Working memory is finite, for people and for models. This is the human analog of `principle-guard-the-context-window`. Code that needs fewer jumps and less held state gets changed correctly more often.

**Pattern:**
- **Collapse layers** that cost more than they save: wrappers with one caller, adapters with no second implementation (an interface with one implementation that nothing substitutes), indirection that was never needed. Inline them.
- **Make adjacent layers change the abstraction.** A layer that repeats the same methods and arguments as the one below adds load without compression. Collapse pass-through layers.
- **Demand interface compression.** A broad interface that hides little makes readers learn both the interface and the implementation. Prefer boundaries that hide meaningful decisions.
- **Shrink state scope.** Prefer pure functions (returns over mutations), locals over fields, fields over module state, and module state over globals. Derive values instead of keeping copies in sync.
- **Shrink visibility the same way.** The fewer places that can touch a thing, the less a reader must search. A private field, a module-private or package-private item, or an unexported name tells the reader that every writer is in this module or package.
- **Name the invariant at the boundary,** not in every consumer, so the reader learns it once. A distinct type such as `NonEmptyName`, which wraps a string and whose only constructor checks the rule, tells every reader the rule holds. A bare string makes each reader check again.
- Before adding a layer or a piece of state, ask: does it reduce reader load elsewhere by at least as much?

**The test:** can a new reader answer "where does X come from?" and "what can change X?" in under 30 seconds? If not, cut layers or cut state. If answering needs more than one `grep` and a couple of `read`s, the code is too spread out.

Distinct from `principle-laziness-protocol`, which keeps the diff small: a small diff can still add a layer. Distinct from `principle-guard-the-context-window`, which protects your own context while you work.
