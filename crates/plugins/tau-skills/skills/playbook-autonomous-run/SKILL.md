---
name: playbook-autonomous-run
description: "Load when told to drive a task to done without stopping (\"run until done\", \"keep going until the tests pass\", \"going to bed, finish this\"). Makes you state a checkable exit condition, offer it as a /goal, iterate in small verified commits, and keep a decision trail on the canvas."
---

# Autonomous run

You own the exit condition. Define done, then drive to it without stopping.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`. Load each skill a step names when you reach it.

The person is away, so blocking on them wastes the run. Load the `principle-never-block-on-the-human` skill. Reversible work (code, fixes, splitting a task, a choice with a sensible default): decide, proceed, and record the assumption in the trail. Only a decision the person alone can make (product direction, a tradeoff between their preferences, anything irreversible or outward-facing such as deleting data or sending a message) goes to `ask`, with concrete options and the recommended one first. `ask` blocks the turn, so finish every other piece of work first. Pushing and pull requests are the person's actions. Never end a turn with a question in prose.

## Steps

1. **State the exit condition.** Write it as something you can check by running a command or reading a result: "the repository's test command passes with 0 failures", "the repro script exits 0", "p50 under 30 ms on the bench". Offer it to the person in one line as the exact text to type: `/goal <condition>`. You cannot set a goal yourself. Once they set it, tau checks every stop against it and sends you back until it holds. Do not wait for them to type it. Start.
2. **Open the trail.** Create one canvas item with `canvas_write`, titled after the task, holding the exit condition and a table with the columns `when | decision | why | evidence | result`. Append a row with `canvas_edit` at each decision point: a fork chosen, a unit finished with its check, a revert and its trigger, a blocker, a side fix. Evidence is a pointer (a commit id, a path, a command), never a paragraph. Rows are append-only. A wrong call gets a new row that supersedes it.
3. **Pick the wake-up for each wait.** A long command: start it with `exec_command` and `notify: true`, then end the turn. tau starts a turn when it exits. Waiting on something outside (a job elsewhere, a time of day): `schedule {name, note, in}` with a note that says what to check and what to do next, then end the turn. Size the interval to when the result is worth checking. Use `scheduled` to see what is pending and `unschedule` to drop a wake-up you no longer need. On the main chat, a sub-agent's landing also starts a turn. On a sub-agent, neither `schedule` nor `notify` works: poll the process with `write_stdin` instead.
4. **Iterate in small verified units.** Load the `principle-sequence-verifiable-units` skill. Each iteration makes the smallest change the evidence justifies, then checks it against the exit condition or the nearest check that predicts it. If it moved things forward, `vcs_commit` it with a Conventional Commits message. If it did not, revert it in full with `vcs_restore` on the paths you touched, or `vcs_undo` if you already committed it. A change that "might help" gets reverted, not left to ride. On the main chat, independent units may go to sub-agents: commit first, `spawn` each with the whole scope and a one-line check it must pass, end the turn, and review each landed diff with `vcs_show`.
5. **Own mid-run discoveries.** A broken skill, a related bug, a flaky check, a tooling failure: fix it yourself as its own commit, record it in the trail, and return to the exit condition. Do not park reversible work for the person.
6. **Keep the context lean.** A long run fills the context. Load the `principle-guard-the-context-window` skill. Process bulk output with scripts or `codemode`. Keep durable state in commits and the trail, so a later turn or a fresh chat can resume with the `playbook-session-pickup` skill.
7. **Stop only when the condition holds.** A plateau is not a stop: change approach and push past it. Never relax the condition to declare victory. A real dead end (every approach tried fails for a reason you can name, or the next step is a call only the person can make) is a stop: record it with its evidence, and use `ask` if a decision unblocks it. Cancel wake-ups you no longer need with `unschedule`.
8. **Audit the trail.** Before you reply, check that each row maps to something that really happened and its evidence resolves (the commit exists, the path holds what the row says). Add rows for any pivot you did not record. Never edit or delete old rows. On a chat, `vcs_land` last, with everything committed.

## Reply

Give the exit condition and its final state (checked by running it now), iterations run, what landed (commits), what was reverted and why, side fixes, and the canvas title of the trail. End with an "Attention" section: rows with weak evidence, checks you skipped, choices that look risky in hindsight, and assumptions the person should confirm. "Nothing to flag" is a valid answer.

Write short declarative sentences. Label each claim measured, inferred, or guess. Never hand the person a check you could run yourself. Name each principle that changed a decision.
