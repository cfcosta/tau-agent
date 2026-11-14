---
name: playbook-investigation
description: "Load when the task is a read-only question: how does X work, why was Y built this way, are we sure about Z, should we do X or Y. Makes you answer from cited evidence in the code and history, with a tradeoffs table for a decision, and change no files."
---

# Investigation

You own the answer. The deliverable is a cited explanation or a recommendation. It is never a code change.

## How to run it

Copy the steps below into your working notes as a numbered checklist and tick each one off as you finish it. A step you do not do stays in the list as `skip: <reason>`.

Change no files. There is nothing to commit at the end. If the question turns out to precede a change the person asked for, finish the answer first, then load `playbook-bug-fix` or `playbook-feature` and start that playbook's checklist. If they only asked the question, answer it and stop.

Stay in this playbook when the answer is a cited fact. Do not build a prototype to answer a question the code and its history can answer.

## Steps

1. Load the `how` skill and run it over the subsystem the question is about. If the question is about motivation ("why is it like this?", "can we remove it?"), also load the `why` skill. `why` reads history through `vcs_log` and `vcs_show` and `grep` over the code. Line-level blame is not available, so say so when the answer would need it.
2. Keep bulk out of your context. Use `codemode` to run many reads or greps and return a small result. On the main chat, when the question spans several independent subsystems, you may `spawn` one sub-agent per subsystem with `land: false` and a brief that says "do not edit files" in words and names what to report. Let them fork your conversation (no `fresh`), so they start from the question and what you already found. Spawn them all, then end the turn. Their reports arrive in later turns. Treat each report as a claim and check the parts your answer rests on yourself. On a chat, read the subsystems yourself one after another. Load `principle-guard-the-context-window`.
3. Check each claim you will make against the code or a run. Run the code when reading it leaves the behaviour unclear. Cite file paths and line ranges, commits by id, and command output you produced in this run.
4. Write the answer in the shape the question needs.
   - For "how does X work", use the `how` output: Overview, Key Concepts, How It Works, Where Things Live, Gotchas.
   - For "should we do X or Y", give a recommendation and a tradeoffs table, one row per option and one column per criterion that matters to the person.
   - For "are we sure?", give your real judgment and the reasons. If the premise is wrong, say so and say what is true instead. Load `principle-attack-the-premise` when the question rests on an assumption you can test.
   - A long answer goes on the canvas with `canvas_write`, with Mermaid for any diagram. The reply then gives the conclusion and points to the canvas item.
5. Load the `humanizer` skill and apply it to the answer before you send it.

## Rules that hold throughout

- No is an acceptable answer. If the person asks whether to do something and it does not earn its place, say that and why. Agreement is not the default.
- Do not `ask` the person for a fact you can find by reading or running something. Use `ask` only when the question needs a preference only they hold, with concrete options and your recommended option first, labelled "(Recommended)". Never end the turn with a question in prose. On a sub-agent, decide and say what you assumed.

## Reply

- Short declarative sentences. No em dashes.
- Every claim carries its evidence or a label in the same sentence: measured (you ran it), inferred (you read it and reasoned), or guess. A prediction or a cause you did not see is a guess.
- Never cite a file, commit, or link you did not read or produce in this run.
- Never hand the person a check you could run yourself. Run it.
- Name each principle that shaped the answer and what it changed.
- Content: the investigation output from step 4. For a recommendation, lead with the verdict, then the table, then what would change your mind.
