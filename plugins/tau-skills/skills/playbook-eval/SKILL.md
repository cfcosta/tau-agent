---
name: playbook-eval
description: "Load when you must test how a skill, prompt, or instruction change affects agent behaviour before adopting it. Makes you fix a rubric first, run fresh sub-agent candidates per variant, relabel their outputs, have a fresh judge score them blind, and say how far the result can be trusted."
---

# Eval

You own the experiment design. Plan, run, blind the judge, judge, synthesize.

If the repository has its own eval suite (an eval package, a harness that starts fresh agents against fixtures), use it instead. It is repeatable and already calibrated.

## Who sees what

- Candidates run with `fresh: true` and `land: false`. Fresh, because the variant text must be the only instruction they see: a forked candidate would see the other variant, the rubric, and that this is an eval. `land: false`, because their work is evidence, not something to keep. Their commits stay beside yours and never land, so they cannot clash and can all work at the same path.
- The judge runs with `fresh: true` and `land: false`. It sees only the rubric and the relabelled outputs, so it is blind to which variant produced which.
- You are not blind. You wrote the variants and spawned the candidates. Your own scores are a second reading, and the judge's are the blind one.
- Grade from what the candidates produced (files, commits, test results), never from what they say they did.

tau drops kept work at its next start. Judge and read everything in this session.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`. Load each skill a step names when you reach it.

Only the main chat can `spawn`. On a chat or a sub-agent you cannot produce candidates. Running the variants yourself in one conversation is not an eval. Say so, and `ask` whether to move the eval to the main chat.

## Steps

1. **Frame.** Name the variant under test (the old and new skill text, or two prompts) and the behaviour that counts as success. Write a rubric of 3 to 6 concrete criteria the judge can check from the outputs alone, each scored 0, 1, or 2, for example "the fix commit comes after a commit with a failing test". Avoid criteria that need the candidate's self-report. Keep it in your notes, written before any candidate runs, so the criteria cannot drift toward an output you liked.
2. **Prepare the place.** Plant what an organic task would have at one path, for example `sandbox/<task>/`: a project skeleton, fixtures. Commit the setup. You must commit before you spawn, and each candidate starts from your newest commit.
3. **Write one organic task.** Write what a person would type. State the goal, not the meta. Do not ask the candidate to list the skills, principles, or files it used, since that changes what it does. Each candidate's task is that text plus its variant, given as the instructions to follow, and nothing else. If the variant must be a file the agent finds by itself (the repository's instructions file, a repository skill), commit variant A in place and spawn its candidates, then commit variant B in place and spawn its candidates; restore the original in a commit of your own afterwards. Add: "Work under `<path>`. Commit your work. End with a one-line reply."
4. **Spawn the candidates.** Run at least 3 per variant (up to 8 sub-agents at once), so one lucky run does not decide it. Give different `effort` levels only if effort is part of what you are testing. Spawn them all, then end the turn. Each report starts a turn for you and names the candidate's changes. Record each candidate's variant and change ids, and do not read the outputs yet. End the turn again until all have reported.
5. **Relabel.** Follow `references/relabel.md` (read it from this skill's folder). It gives each candidate a shuffled label, so the judge gets labels and change ids, or scrubbed copies, and nothing that names a variant.
6. **Judge.** Spawn one judge with `fresh: true` and `land: false`. Its task holds the organic task, the rubric, and the labelled candidates from step 5, and says "Do not edit files. Commit nothing." It scores every label on every criterion in one pass on one scale and gives a short reason per score.
7. **Read everything yourself.** While the judge runs, read every candidate's changes end to end with `vcs_show`, run their tests if the rubric needs it (restore their files with `vcs_restore {paths, from}` to run them, and put your own back with `vcs_restore {paths}` after), and score them yourself. When the judge reports, open the mapping. Where you and the judge disagree, the rubric is ambiguous or you are biased. Name which.
8. **Synthesize and clean up.** Count each criterion per variant (for example "failing test first: 4 of 4 with B, 1 of 4 with A"). A one-run difference is noise. Call it inconclusive when the counts overlap. Delete the sandbox setup in one commit of your own. The candidates' work never landed, so there is nothing else to delete.

## Reply

Give the variant under test, the rubric, the per-candidate notes, the judge's scores, your scores, the per-variant counts, and a recommendation: adopt, reject, or rerun with a sharper rubric. Say in one or two sentences what the judge could still infer (style, structure, anything the relabelling missed) and that your own scores were not blind.

Write short declarative sentences. Label each claim measured, inferred, or guess. Never hand the person a check you could run yourself. Name each principle that changed a decision. Load the `principle-explain-the-number` skill before you trust a count.
