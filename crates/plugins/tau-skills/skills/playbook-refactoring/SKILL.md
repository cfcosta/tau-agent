---
name: playbook-refactoring
description: "Load when the task is to change code's structure without changing its behaviour: rename, extract, inline, dedupe, move, reshape an API. Makes you pin behaviour first, subtract first, move in small green steps, migrate callers and delete in one wave, and revert if reader load did not drop."
---

# Refactoring

You own the contract. The structure changes. The behaviour does not.

This playbook is distinct from `playbook-feature`, which adds behaviour, and `playbook-bug-fix`, which corrects it. If the cleanup reveals a missing feature or a real bug, split it out: land the structural change first against the pinned contract, then handle the rest with its own playbook. A redesign is allowed, but name it as one and run `playbook-feature`. Large or cross-cutting structural work goes to the `figure-it-out` skill. This playbook is for a focused to medium change.

## How to run it

Copy the steps below into your working notes as a numbered checklist and tick each one off as you finish it. A step you do not do stays in the list as `skip: <reason>`.

## Steps

1. Pin the behaviour first. Load the `how` skill and run it over the affected code to learn its contract. Then write a characterization test, a snapshot, or an equivalence harness that captures what the code does today, before any structure moves. If the area has no coverage, write the pin before you touch it. A type check and a lint are not a pin. Load `principle-test-behavior-not-implementation` so the pin survives the reshape. Commit the pin on its own.
2. Name the structure the code is missing. Load `principle-model-the-domain`. Leave boring code alone when its shape is already clear and local. The reshape must delete branches or invalid states, not add indirection.
3. Name the target shape. Write down the module layout, types, and call graph the code would have if you built it today. Load `principle-foundational-thinking` and `principle-redesign-from-first-principles`. If the target crosses a function boundary, load the `architect` skill to explore the shape before the move.
4. Subtract before you add. Delete dead code, collapse one-caller wrappers, drop redundant validators, and remove orphan references before you introduce the new shape. Load `principle-subtract-before-you-add` and `principle-laziness-protocol`. The smallest change that reaches the target ships. Revert a speculative cleanup that "might help". Commit the subtraction on its own.
5. Move in small behaviour-preserving steps, each one keeping the pin green. For an API reshape, migrate every caller and delete the old API in the same wave (load `principle-migrate-callers-then-delete-legacy-apis`). No compatibility shims. No parallel old and new paths. For a mechanical edit across many files, write a script or codemod and run it, so the reviewer can rerun it (load `principle-build-the-lever`). After every rename, `grep` the whole repository for the old name. Renames miss usages in strings, docs, config, and back-references. On the main chat you may `spawn` a sub-agent for a part with disjoint files. Commit first, brief it with the paths, the names that move, and the pin that must stay green, then end the turn. When its commits land, review them with `vcs_show` as claims to verify. Otherwise do the edits yourself.
6. Prove the behaviour did not change, on the real artifact, not "it compiles" (load `principle-prove-it-works`). Run the pin. For a larger reshape, also run an equivalence check: a script that diffs old and new outputs, a recorded baseline replayed against the new code, or a smoke run on the matching surface with the repository's `verify-*` skill or the commands its instructions give.
7. Confirm the change is worth keeping. The measure is lower reader load (load `principle-minimize-reader-load`). Count what a reader must hold to follow the code before and after: layers, hidden state, files to open, branches. If the diff does not lower reader load somewhere, revert it. To revert committed steps, restore the paths from the commit before the refactor with `vcs_restore {paths, from}` and commit that with a message that says why.
8. Commit and finish. The order is the pin, the subtraction, the reshape, then any follow-on cleanup, each green before the next (load `principle-sequence-verifiable-units`). There is no split or squash tool, so commit each step as soon as it is green, using `vcs_commit {paths}` when other files are dirty. Messages follow Conventional Commits (`refactor(scope): subject`, `test(scope): ...` for the pin, imperative, no trailing period). On a chat, finish with `vcs_land`. On the main chat, commit and you are done. On a sub-agent, commit, and your commits land on the caller when you end.

## Rules that hold throughout

- Decide reversible things yourself, then say in the reply what you assumed and why. Use `ask`, with concrete options and the recommended one first, only for a call only the person can make, such as deleting a public API that outside users consume. Never end a turn with a question in prose. On a sub-agent, decide and report the assumption.
- Keep a comment only for a non-obvious why the code cannot show.

## Reply

Load the `humanizer` skill for the reply. Short declarative sentences, no em dashes. Every claim carries its evidence or a label in the same sentence: measured, inferred, or guess. Never cite what you did not read or produce in this run, and never hand the person a check you could run. Name each principle that changed a decision and what it changed.

Content: the structure that changed. The pin you held it against. The equivalence proof. The reader-load delta. What you committed and what you reverted. No new behaviour.
