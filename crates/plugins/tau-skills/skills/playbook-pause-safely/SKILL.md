---
name: playbook-pause-safely
description: "Load when the person asks you to pause or stop for now so the work can resume later, or when your context is about to run out mid-task. Makes you stop at a clean boundary, commit with an honest message, leave a resume note on the canvas, and schedule a wake-up if needed."
---

# Pause safely

You own a clean stop. Leave a checkpoint that a cold start can resume from. The complement is the `playbook-session-pickup` skill.

Pause only when asked, or when the context will not hold the next step. On "keep going", "going to bed, keep going", or "don't stop", do not pause: load the `playbook-autonomous-run` skill.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`.

A pause is a reversible act. Make every choice in it yourself and say what you chose. Do not `ask` the person how to pause. Never end a turn with a question in prose.

## Steps

1. **Stop at a clean boundary.** Finish the current atomic step if it is small, or back out of it. Start nothing new. Do not `spawn`. There is no tool to stop a running sub-agent. Each lands when it finishes, or, if spawned with `land: false`, is kept beside your work. List the running ones in the resume note. Take what you need from kept work now with `vcs_restore {paths, from}` and commit it: tau drops kept work at its next start.
2. **Take no irreversible or outward-facing action to pause.** Pushing is the person's action. On a chat, do not `vcs_land` a paused stack: landing is for finished work.
3. **Clean the working copy.** Remove throwaway edits (debug prints, local instrumentation, scratch files in the tree) with `vcs_restore` on those paths. Check with `vcs_status` and `vcs_diff`.
4. **Commit the work.** `vcs_commit` with a Conventional Commits message that says the truth: what this commit does, that the work is paused, and what is not done yet. If the tree does not build or tests fail, say so in the message in one plain line, for example "paused: `parse_header` is half written, the wire package's tests fail". Never leave a broken commit that reads as finished. Commit wherever a reviewer would want a boundary, which may be more than one commit.
5. **Write the resume note.** Put it on the canvas with `canvas_write`, titled "Resume: <task>". A cold start needs:
   - The intent: the goal in the person's words, and any `/goal` condition still set.
   - Progress: what landed (commit ids), what is verified and how.
   - State: what is uncommitted or broken, which sub-agents are still running, and the change ids of kept work you have not taken.
   - Next steps: the first action on resume, then the rest in order.
   - Key files and gotchas: the paths that matter, the trap you found.
   If a decision trail already exists on the canvas, point at it instead of repeating it, and append a "paused" row to it with `canvas_edit`. Facts that outlive this task (a repository quirk, a tool's flag) go to `memory_write`. Task progress does not.
6. **Schedule the resume if the chat should continue itself.** When the person wants the work to pick up later without them, `schedule {name, note, in|on}` with a note that names the canvas resume note and the first action. Not available on a sub-agent: there, end with the resume note in your final answer, since it reaches the caller.

## Reply

Say where you stopped, what is on disk (commit ids, the canvas note's title) versus what was only in your head and is now in the note, whether the working copy is clean, whether the last commit builds and passes, any wake-up you scheduled, and the first action on resume. This is a pause, not a final report.

Write short declarative sentences. Label each claim measured, inferred, or guess.
