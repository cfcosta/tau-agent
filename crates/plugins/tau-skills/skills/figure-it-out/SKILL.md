---
name: figure-it-out
description: "Load when no playbook fits the task, or for a large migration, an ambitious multi-part change, or work the person will review after stepping away. Makes you design a rigorous workflow first, run each unit as an experiment, and keep a decision trail on the canvas."
---

# Figure it out

When the task matches no `playbook-*` skill, design one. Do the same for a large or cross-cutting effort even when `playbook-feature` fits, and for work the person will review after stepping away. The first deliverable, before any code, is the workflow itself: phases that scale rigor to the task, run each unit as an experiment, and leave a trail the person can audit.

## Start

Write a numbered checklist in your working notes, or on the canvas for a long run. Its first item is to read the principles index in your base prompt and load each principle that applies to this task. For most runs that includes `principle-prove-it-works`, `principle-sequence-verifiable-units`, `principle-foundational-thinking`, and `principle-never-block-on-the-human`. Then add phases A to E below as items. A step you do not do stays in the list as `skip: <reason>`.

## Phase A: Frame

Ground yourself first (load the `how` skill over the code the task touches), then commit to a frame. Do not start the run until you can state these.

- Done, as a falsifiable condition. A command and its expected result, a measurable threshold, or an observable state on the real product. "Works well" is not a condition; "`<test command>` passes and no call site of `old_api` remains (`grep` finds none)" is. Load `principle-prove-it-works`.
- Scope, quantified. Rough units of work and effort, and the blockers that grounding found.
- The rigor level, biased high. One-way doors and a wide blast radius get more. Reversible, low-stakes steps get less. Rigor means gates and artifacts, not trying harder. Load the `blast-radius` skill when you cannot tell what the change touches.

Offer the person the done condition as a goal they can type, with the exact text, for example `/goal <test command> passes and grep finds no call to old_api`, with the repository's real test command in place of `<test command>`. You cannot set a goal yourself. Once set, tau sends you back at each stop until it holds.

Put the framing on the canvas with `canvas_write`. Reversible work proceeds without approval. A run of several hours earns one checkpoint. If the frame holds a choice only the person can make (cutting scope, a one-way door, spending real money or time), `ask` with concrete options and your recommendation first. Otherwise state your assumptions and start. Never end a turn with a question in prose.

## Phase B: Design the workflow

- Split the work into atomic units, each landable on its own. Put the riskiest unknown first. Put scaffold and verification before features (load `principle-foundational-thinking`).
- Build the verification harness before the work. Capture the baseline from the code as it is now, so each check reads as "old value, new value". If the repository has a `verify-*` skill, use it; if driving the real surface needs one, load `verification-skill` to make it.
- For a one-way-door design decision, load the `architect` skill, which runs `arena`. Skip it for mechanical work whose shape is already clear. A second arena over a settled design is waste (load `principle-laziness-protocol`).
- Decide what fans out. Parallelize only across real seams, where units touch disjoint files (load `principle-separate-before-serializing-shared-state`). Do not over-fan. On the main chat you may `spawn` up to 8 sub-agents, each with its own workspace, an `effort` that fits its job, and a brief that names its paths, its data shape, and its check. Commit before you spawn, spawn every unit that can start now, then end the turn; their commits land on yours and start your next turn. On a chat or a sub-agent, run the units yourself one after another.
- Write the designed phase list on the canvas. That list is what the person reviews.

Then execute the design. Add its units to the checklist between Phase C and Phase D. Run each under the Phase C loop, and append a Phase D row as each unit lands, not all at the end.

## Phase C: Run the loop

Each unit is an experiment. State the hypothesis. Make the smallest change. Measure it against the done condition on the real artifact. Keep it if it advanced, revert it if it did not (`vcs_restore` for uncommitted work). Verify each unit before you start the next (load `principle-sequence-verifiable-units`), and commit each one when it is green with a Conventional Commits message.

- Verify by inspecting the artifact, never by a self-report. When something passes too easily, suspect the way you observed it before you trust the system.
- Treat work a sub-agent landed as claims to verify. Read its commits with `vcs_show` and rerun its checks yourself. If it gamed the check, reset its work and tighten the brief. If the check itself is wrong, fix the check in its own commit instead of routing around it. A sub-agent that forked your conversation shares your blind spots. When you need an independent check, spawn a reviewer with `fresh: true` and `land: false`, and write into its task everything it needs: the done condition, the commit ids, the check to run, and "Do not edit files. Commit nothing."
- A verdict is VERIFIED, NOT VERIFIED, or INCONCLUSIVE. Inconclusive is not a pass. Do not hide a negative result.
- If you report or act on a number, load `principle-explain-the-number` first.

## Phase D: Keep the decision trail

Keep the trail on the canvas whenever the run is long or the person will review it later, which is the usual case here. Create one item with `canvas_write` (title it after the task) and append a row with `canvas_edit` each time a unit lands or a decision changes the plan. Columns: time, phase, decision, why, evidence, result. Evidence is a pointer you can follow again: a commit id, a file path, a command and its outcome. The trail is append-only. When a later finding overturns an earlier row, add a row that supersedes it and leave the old one.

## Phase E: Verify and hand back

Check the whole against the Phase A condition on the real product, not only the harness. If a correction recurred during the run, encode it as a check, a lint, a test, or a script, in its own commit (load `principle-encode-lessons-in-structure`). Commit everything. On a chat, finish with `vcs_land`. On the main chat, commit and you are done. On a sub-agent, commit, and your commits land on the caller when you end.

## Reply

Load the `humanizer` skill for the reply. Short declarative sentences, no em dashes. Every claim carries its evidence or a label in the same sentence: measured, inferred, or guess. Never cite what you did not read or produce in this run, and never hand the person a check you could run. Name each principle that changed a decision and what it changed. For each assumption you made instead of asking, say what you assumed and why.

Content: the workflow you designed. The rigor level and why. The `/goal` condition you offered. The canvas item that holds the trail. What is VERIFIED against the done condition, and what is still open.
