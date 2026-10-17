---
name: playbook-eval
description: "Load when you must test how a skill, prompt, or instruction change affects agent behaviour before adopting it. Makes you fix a rubric first, run sub-agent candidates per variant, relabel their outputs by script, judge against the rubric, and say how far the result can be trusted."
---

# Eval

You own the experiment design. Plan, run, blind what you can, judge, synthesize.

If the repository has its own eval suite (an eval package, a harness that starts fresh agents against fixtures), use it instead. It runs agents that have not seen your conversation, which this playbook cannot.

## What this playbook cannot do

Every `spawn` forks your conversation. A candidate sees the variant under test, the rubric, and that this is an eval. A judge sees which candidate got which prompt. True blinding is not possible with sub-agents today. Say so in the reply. The steps below reduce the bias:

- Write and commit the rubric before any candidate runs, so the criteria cannot drift toward an output you liked.
- Ask candidates for a one-line final reply, so their landing reports carry little into your conversation before judging.
- Relabel and scrub the outputs with a script whose mapping you do not read until the judging is done.
- Grade from what the candidates produced (files, commits, test results), never from what they say they did.

For a result that matters, the strongest fix is a chat the person starts. It does not fork your conversation. Offer it with `ask`: the person opens fresh chats and types the prompt for each variant.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`. Load each skill a step names when you reach it.

Only the main chat can `spawn`. On a chat or a sub-agent you cannot produce candidates. Running the variants yourself in one conversation is not an eval. Say so, and `ask` whether to move the eval to the main chat or into the person's own fresh chats.

## Steps

1. **Frame.** Name the variant under test (the old and new skill text, or two prompts) and the behaviour that counts as success. Write a rubric of 3 to 6 concrete criteria the judge can check from the outputs alone, each scored 0, 1, or 2, for example "the fix commit comes after a commit with a failing test". Avoid criteria that need the candidate's self-report. Write it to a file and `vcs_commit` it.
2. **Prepare each candidate's place.** Give each candidate its own path, for example `sandbox/<task>/<n>/`, and plant what an organic task would have there: a project skeleton, fixtures, the variant's text in a file the candidate is told to follow. Commit the setup. You must commit before you spawn.
3. **Write one organic task.** Write what a person would type. State the goal, not the meta. Do not ask the candidate to list the skills, principles, or files it used, since that changes what it does. Each candidate gets the same task text and differs only in which variant file it follows. Add: "Write only under `<path>`. Commit your work. End with a one-line reply."
4. **Spawn the candidates.** Run at least 3 per variant (up to 8 sub-agents at once), so one lucky run does not decide it. Give different `effort` levels only if effort is part of what you are testing. Spawn them all, then end the turn. Each landing starts a turn for you. Do not read the outputs yet. End the turn again until all have landed. Their commits land on top of yours. Changes that clash land as conflicts. Resolve them by keeping each candidate's own path.
5. **Relabel.** Run the script from `references/relabel.md` (read it from this skill's folder) through `exec_command`. It copies each candidate's path to a shuffled label, scrubs the variant names you pass it, and writes the mapping to a file you leave unread.
6. **Judge.** Spawn one judge sub-agent told "do not edit files". Give it the rubric path and the relabelled folder only. It scores every label on every criterion in one pass on one scale and gives a short reason per score. It forks you too, so its value is a second, fixed-rubric reading, not independence.
7. **Read everything yourself.** After the judge lands, read every candidate output end to end. Then read their commits with `vcs_log` and `vcs_show`, run their tests if the rubric needs it, and score them yourself. Only now open the mapping. Where you and the judge disagree, the rubric is ambiguous or one of you is biased. Name which.
8. **Synthesize and clean up.** Count each criterion per variant (for example "failing test first: 4 of 4 with B, 1 of 4 with A"). A one-run difference is noise. Call it inconclusive when the counts overlap. Delete the sandbox paths and scratch copies in one commit of your own.

## Reply

Give the variant under test, the rubric, the per-candidate notes, the judge's scores, your scores, the per-variant counts, and a recommendation: adopt, reject, or rerun with a sharper rubric. State the blinding limits above in one or two sentences.

Write short declarative sentences. Label each claim measured, inferred, or guess. Never hand the person a check you could run yourself. Name each principle that changed a decision. Load the `principle-explain-the-number` skill before you trust a count.
