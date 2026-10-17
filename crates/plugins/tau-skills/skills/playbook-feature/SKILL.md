---
name: playbook-feature
description: "Load when the task is to build new behaviour or change what existing code does: a feature, an option, a new command or endpoint. Makes you map the subsystem, name the data shape, explore designs, plan the parallel work, verify on the real surface, and commit in small ordered units."
---

# Feature

You own the design. Plan it, review it, verify it.

This playbook is for a focused feature. A large or cross-cutting effort, such as a migration across many call sites or work the person will review after stepping away, goes to the `figure-it-out` skill instead. A behaviour-preserving reshape goes to `playbook-refactoring`. A design question you can settle by trying it goes to `playbook-prototype` first.

## How to run it

Copy the steps below into your working notes as a numbered checklist and tick each one off as you finish it. A step you do not do stays in the list as `skip: <reason>`.

## Steps

1. Map the ground. Load the `how` skill and run it over the subsystem the feature touches.
2. Name the data shape. Before any logic, write down the data the feature adds or changes and the structure that organizes it: a state machine over scattered booleans, a table or registry over branching, a typed model over repeated shape assumptions. Load `principle-model-the-domain` and `principle-foundational-thinking`. If the feature is a new requirement on an existing design, load `principle-redesign-from-first-principles` and design as if it had been there from day one. If dead weight sits where the feature goes, load `principle-subtract-before-you-add` and remove it first. For product or UX tradeoffs, load `principle-experience-first`.
3. Explore the design. If the change crosses a function boundary, load the `architect` skill for parallel design exploration. Skip it, with the reason, only for a change that stays inside one function.
4. Write the throughput checkpoint as four items. A dimension that does not apply keeps its item as `n/a: <reason>`.
   - Blocking first steps. What must exist before anything can run in parallel: shared types, a schema, the test harness.
   - Independent workstreams. Parts that touch disjoint files or layers and can proceed in parallel. Shared writes serialize.
   - Shared mutable state. Split the target before you serialize access to it (load `principle-separate-before-serializing-shared-state`). Serialize only for a real invariant.
   - Smallest safe decomposition. If one worker is best, say why.
5. Implement. When the implementation admits several valid shapes (error handling, the abstraction layer, the test structure), load the `arena` skill and let the candidates surface the alternatives. Do not skip this to save lines; the gain is a reviewed comparison, not a smaller diff. Otherwise, on the main chat you may `spawn` a sub-agent for each independent workstream from step 4. Commit first, brief each with its file paths, the named data shape, and its success check, spawn them all, then end the turn. When their commits land, review each with `vcs_show` as claims to verify, not as facts. Code-coupled work stays with you. On a chat or a sub-agent, implement everything yourself, one workstream after another. Make surgical edits. When you improve a shared primitive, update every consumer and verify each.
6. Verify on the matching surface. That is where the person will use the feature: the CLI command, the running app, the API. Drive it with the repository's `verify-*` skill if it has one; otherwise run what the repository's instructions say. For a desktop UI, take screenshots from a headless compositor that the repository or its verify skill sets up, never from the person's real display, and put them on the canvas with `canvas_show`. Load `principle-prove-it-works`. A wrong-surface or inconclusive result is not a pass. Say so plainly. If you report a number, load `principle-explain-the-number` first.
7. Commit in small ordered units. Load `principle-sequence-verifiable-units`. Each commit builds, passes its check, and tells the next step of the story: scaffold and types, then behaviour, then wiring and polish. There is no split or squash tool, so pick the boundaries before you edit, commit each unit as soon as it is green, and use `vcs_commit {paths}` to commit only that unit's files. Messages follow Conventional Commits (`feat(scope): subject`, imperative, no trailing period). Load `technical-writing` for any commit body.
8. If the design is contested, by the person, by a reviewer, or by your own doubt about a one-way door, load the `interrogate` skill over the commit stack before you finish.
9. Finish. On a chat, finish with `vcs_land`. On the main chat, commit and you are done. On a sub-agent, commit, and your commits land on the caller when you end.

Rewrite the checkpoint when a phase boundary changes the plan.

## Rules that hold throughout

- Decide reversible things yourself, then say in the reply what you assumed and why. A fork you can settle by running something is not the person's to answer; settle it with `playbook-prototype`. Use `ask`, with concrete options and the recommended one first, only for a call only the person can make: product direction, a tradeoff between their preferences, anything irreversible or outward-facing. Never end a turn with a question in prose. On a sub-agent, decide and report the assumption. Load `principle-never-block-on-the-human`.
- No is an acceptable answer. If added scope does not earn its place, say so.
- Keep a comment only for a non-obvious why the code cannot show.

## Reply

Load the `humanizer` skill for the reply. Short declarative sentences, no em dashes. Every claim carries its evidence or a label in the same sentence: measured, inferred, or guess. Never cite what you did not read or produce in this run, and never hand the person a check you could run. Name each principle that changed a decision and what it changed. Lead with what changes for the people who use the code, then what the next maintainer inherits.

Content: what you built. What you chose and why, with a table for design alternatives. The throughput checkpoint. How you verified it. The commits. Open decisions.
