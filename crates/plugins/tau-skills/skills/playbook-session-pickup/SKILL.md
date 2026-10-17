---
name: playbook-session-pickup
description: "Load when resuming or taking over earlier work, yours or another agent's (\"pick this up\", \"continue where it stopped\", a wake-up note, a resume note on the canvas). Makes you rebuild state from commits, canvas and memory, trust the prior trail, and resume without redoing work."
---

# Session pickup

You own the resume point. Read the prior trail. Do not redo it.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`. Load each skill a step names when you reach it.

Reversible choices (which open item to take first, whether a prior conclusion stands): decide and say why. Use `ask` only when the trail leaves a decision open that only the person can make, with the recommended option first. Never end a turn with a question in prose.

## Steps

1. **Find the prior trail.** tau keeps no transcript files. The trail is in:
   - Commits: `vcs_log` for the recent stack, `vcs_show` on each commit that belongs to the work. Commit messages carry the intent and often the numbers.
   - The working copy: `vcs_status` and `vcs_diff` for uncommitted changes the last run left.
   - The canvas: `canvas_read` for a resume note, a decision trail, or a hillclimb table.
   - Memory: `memory_search` for facts recorded about this work or this repository, then `memory_read` on the hits.
   - Wake-ups: `scheduled` for pending notes, and the note of the wake-up that started this turn, if one did.
   - A sub-agent already holds its caller's conversation. Read it, do not search for it.
   Read the most recent items first, then scan back for the decision points. If the trail is long, load the `principle-guard-the-context-window` skill and reduce it to a short timeline in your notes.
2. **Rebuild the state.** Write down the goal, the decisions made and why, what landed, what is uncommitted, what was verified and how, the open items, and any `/goal` condition still set. The prior trail is authoritative input. Resist the urge to re-derive it.
3. **Diff done against pending.** Compare what landed with what was planned. Name the resume point as one concrete next action. Do not re-run the prior repro or redo finished work. A "let me verify from scratch" pass treats an authoritative trail as untrustworthy.
4. **Pick the verdict and route.** Choose one: continue the work, finish a recommendation that is already supported, ratify or override a prior conclusion (with the evidence that overrides it), or write a postmortem of a failed run. Then load the playbook that fits the remaining work (for example `playbook-feature`, `playbook-bug-fix`, `playbook-hillclimb`, `playbook-autonomous-run`). This playbook ends there. The routed playbook owns the rest.
5. **Verify inherited claims on the real artifact.** Before you build on a claim such as "tests pass" or "p50 is 33 ms", check it against the original goal by running it. A prior self-report is not proof. Load the `principle-prove-it-works` skill. This is the one check you repeat. It differs from redoing work: you run the check, you do not rebuild what it checks.
6. **Mark the pickup.** If a canvas trail exists, append a row with `canvas_edit` that says this run resumed it and from which point. If a pending wake-up or a resume note is now stale, `unschedule` it or update the note.

## Reply

Say where the prior work stopped, what you inherited and what you redid (ideally nothing), which inherited claims you re-checked and their result, the resume point, the playbook you routed to, and the outcome so far.

Write short declarative sentences. Label each claim measured, inferred, or guess. Cite only commits, canvas items, and memories you read in this run. Never hand the person a check you could run yourself.
