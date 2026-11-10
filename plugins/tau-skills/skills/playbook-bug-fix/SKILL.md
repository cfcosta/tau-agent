---
name: playbook-bug-fix
description: "Load when the task is to fix a defect: something reported as broken, wrong, crashing, or regressed. Makes you reproduce it on the real surface, binary-search the cause with runtime evidence, commit a failing test before the fix, and verify on the same surface."
---

# Bug fix

You own this fix. Plan it, review it, verify it.

Be scientific. Every line you commit traces to runtime evidence. A guard that "might help" is a hypothesis, not a fix, and it does not ship. When evidence refutes a hypothesis, revert the code it motivated, instrumentation included. Ship the smallest change the evidence justifies. Load `principle-fix-root-causes` before you start.

## How to run it

Copy the steps below into your working notes as a numbered checklist and tick each one off as you finish it. A step you do not do stays in the list as `skip: <reason>`.

## Steps

1. Reproduce it yourself on the real surface. The real surface is where the bug shows: the CLI command, the running app, the API call, the failing test. Drive it with the repository's `verify-*` skill if it has one. Otherwise run what the repository's instructions say. For a desktop UI, take screenshots from a headless compositor that the repository or its verify skill sets up, never from the person's real display, and put them on the canvas with `canvas_show`. If it does not reproduce at once, synthesize the trigger, tighten the conditions, or add logging until it fires. Ask the person to reproduce only after you drove the surface as far as it goes, and only with a stated, specific reason it cannot reach the bug. Use `ask` with concrete options for that, never a prose question. Save the failing output verbatim.
2. Binary-search the cause. Write down the candidate hypotheses. Seed them with the `how` skill over the affected subsystem and the `why` skill for regression history (`vcs_log`, `vcs_show`, `grep`; line-level blame is not available). Each pass, test the split that rules out the most remaining space, get runtime evidence, and cross off what it refutes. When program state is unclear, add logging and read it as the code runs. Do not guess. If two fixes in a row failed the same check, they share a premise. Load `principle-attack-the-premise`, write the premise down, and test it before a third fix. For a slow or stubborn hunt, run long commands with `exec_command` and `notify: true`, or `schedule` a wake-up, and suggest the person type `/goal <the repro command> passes` so tau sends you back until it holds. Confirm the surviving mechanism with runtime evidence before you plan the fix.
3. Plan the fix. Fix it where the system first went wrong, not where the symptom shows. A guard is a fix only when the state it handles is legitimate. Before code, name the data shape the fix touches and the structure that organizes it (load `principle-model-the-domain`). If the fix crosses a function boundary, load the `architect` skill first. If the right fix is contested, load `interrogate` before you commit it.
4. Commit the failing test first. Load the `tdd` skill. Write the smallest test that fails for the bug's reason, run it, and commit it on its own with `vcs_commit` before you touch the fix. Skip this step, with the reason, when the test would be expensive, integration-heavy, or unclear, and keep the repro from step 1 as the check instead.
5. Implement the fix. On the main chat you may `spawn` a sub-agent for a part of the fix that is independent of the rest. Commit first, brief it with the paths, the data shape, and the check that must pass, then end the turn. When its commits land, review them with `vcs_show` as claims to verify, not as facts. Otherwise implement the fix yourself. Before you commit, read `vcs_diff` and remove leftover logging and any change a refuted hypothesis motivated.
6. Verify on the same surface. Run the step 1 repro again. It must pass. A unit test shows one branch's behaviour, not that the bug is gone. A wrong-surface or inconclusive result is not a pass. Say so plainly.
7. Commit and finish. Commit in small ordered commits with Conventional Commits messages (`fix(scope): subject`, imperative, no trailing period). The failing test sits before the fix in history (load `principle-sequence-verifiable-units`). There is no split or squash tool, so commit each unit as soon as it is green, and use `vcs_commit {paths}` to commit only that unit's files. On a chat, finish with `vcs_land`. On the main chat, commit and you are done. On a sub-agent, commit, and your commits land on the caller when you end.

## Rules that hold throughout

- Decide reversible things yourself and say in the reply what you assumed and why. Use `ask`, with concrete options and the recommended one first, only for a call only the person can make: product behaviour, a tradeoff between their preferences, anything irreversible or outward-facing. Never end a turn with a question in prose. On a sub-agent, decide and report the assumption.
- Keep a comment only for a non-obvious why the code cannot show.

## Reply

Load the `humanizer` skill for the reply. Short declarative sentences, no em dashes. Every claim carries its evidence or a label in the same sentence: measured, inferred, or guess. Never cite what you did not read or produce in this run, and never hand the person a check you could run. Name each principle that changed a decision and what it changed.

Content: what was broken, from the user's side. The root cause. The fix. How you verified it, with the failing-then-passing repro output pasted verbatim. The commits, by id and message.
