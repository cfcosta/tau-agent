---
name: architect
description: "Load before writing code that crosses a function boundary, for \"architect this\" or \"design this\", or when jumping to code would lock in the wrong shape. Makes you ground in the code, sketch two or more distinct designs as types and signatures, screen them for red flags, then build to the sketch."
---

# Architect

Sketch the types, function signatures, interfaces and data shapes, and module boundaries first, with unimplemented bodies and pseudocode. Compare at least two whole designs. Then fill in the code against the sketch you chose, and stay alert to the moment the sketch stops fitting.

Write the phases into your notes as a numbered checklist and tick each off as you finish it: Ground, Sketch, Agree, Implement, Scrap.

## Phase A: Ground the problem

Build a real mental model of every system the new code touches. Load the `how` skill and run it over those subsystems. Naming a file is not grounding. Produce the explanation `how` prescribes. If the design changes who owns what, or which layer does what, also load the `why` skill and run it on the current shape, so its rationale becomes a constraint and not a guess.

Skip Phase A only when the work is greenfield, with no surrounding system to integrate with.

## Phase B: Sketch

Design it twice. Require at least two structurally distinct candidates before you choose, even when the first looks good enough. They must be whole-shape alternatives, not point fixes inside one shape. This is `principle-exhaust-the-design-space` made concrete.

A candidate is a design package: the caller's usage written first, then the type sketch derived from it (types, signatures, a module map, bodies that only raise a "not implemented" error, in whatever form the repository's language uses), and a rationale shaped by `references/rationale-template.md`.

All candidates come from the same family of models, so the difference has to come from the briefs. Give each candidate a different starting point, for example: one owns the state in a single type and exposes operations on it; one models the domain as data and keeps the operations as pure functions; one starts from the smallest change to the existing modules. Tell each to commit to its shape and not hedge toward the others.

**On the main chat**, load the `arena` skill and run it with the design-sketch task. Pass `references/runner-prompt.md` from this skill's folder as each candidate's brief, with its distinct starting point, the Phase A explanation, and its own path (`prototypes/<task>/<a|b|c>/`). Arena returns one synthesized design. Its synthesis decision fills the rationale's "Synthesis decision" section.

**On a chat or a sub-agent**, `spawn` is refused. Write two sketches yourself, one after another, each under its own path and from its own brief. Write both briefs before either sketch. Then compare them as below and graft.

Screen every candidate against `references/design-red-flags.md` before you choose. Assume the next contributor is an agent that sees only the files it opened, copies the nearest example, and takes the shortest path that compiles. Prefer the design where a change that looks right from one file is right for the whole repository.

Compare the viable candidates on interface depth. Prefer the design that hides more complexity behind a smaller, simpler public interface. A rich interface can keep call chains short by concentrating capability instead of scattering it across layers.

## Phase C: Agree

By default, go straight to implementation with the synthesized design.

Use `ask` for a checkpoint when the person asked for one ("show me before you implement"), or when the candidates differ on a tradeoff only the person can weigh, such as a product behaviour or a public API that others will build on. Give each candidate as an option with one line on what it costs, your recommended one first, labelled "(Recommended)". Put the sketches on the canvas with `canvas_write` so the person can read them. On a sub-agent, which cannot ask, choose, and say in your answer what you chose and why.

The synthesized sketch can land as its own commit either way, the "scaffold first" mode of `principle-foundational-thinking`. Planned breakage while you fill it in is fine, per `principle-outcome-oriented-execution`. For adversarial pressure on the design before you implement, load the `interrogate` skill and run it on the sketch.

If the person pushes back on the shape, at a checkpoint or later, treat that as Phase A evidence. Ground again and run Phase B again before you write more code.

## Phase D: Implement against the sketch

Replace unimplemented bodies with code and pseudocode with logic. The sketch is the contract. A deviation from it is a signal to report, not friction to absorb quietly. If a function needs a parameter the sketch did not foresee, ask yourself whether the sketch was wrong, the requirement was missed, or the implementation is overreaching. Say which in your reply or commit message.

## Phase E: Scrap when the architecture is wrong

If implementation keeps producing friction the sketch cannot absorb, throw the sketch out. Do not bolt fixes onto a wrong design, per `principle-redesign-from-first-principles` and `principle-fix-root-causes`.

The signal is a pattern, not one instance:

- The same shape of workaround appears in unrelated code.
- Several unrelated edge cases each need a special-case branch.
- Types need escape hatches to compile: unchecked casts, a forced unwrap of a value the design said always exists, optional fields that are always set in practice, a dynamic "any" type, an unsafe block the design did not call for.
- The "we need a lock" reflex appears where the sketch said the state was not shared.
- Callers must know the abstraction's internal rules to use it.
- Two or more independent Phase D deviations have the same shape.

Use judgment. A few edge cases do not condemn an architecture. Some problems are complex. Complexity in the data is not complexity in the design.

When you scrap:

1. Run the `how` skill again over what has been built.
2. Redesign as if the new constraints had been there on day one, per `principle-redesign-from-first-principles`.
3. Subtract before you add, per `principle-subtract-before-you-add`. The new sketch should be smaller than the old one before it grows.
4. Return to Phase B.

## Outputs

The caller's usage, written first, and the type sketch derived from it. One file of new types and signatures for a small change. A module map plus type definitions for larger work. The rationale ships with it, shaped by `references/rationale-template.md`, including the usage and the synthesis decision.
