---
name: how
description: "Load when asked how X works, for a walkthrough before you change code, or where something should live (which module or package owns it, is this the right layer). Makes you explore angle by angle and write an architectural explanation. For motivation and history, load `why`."
---

# How

Answer "how does X work?" at the level a senior engineer needs when they join a subsystem. The reader should come away with a working mental model. The answer should not read like annotated source code.

Change no files while you do this. There is nothing to commit.

## 1. Size the question

If the scope is unclear, state how you read it and go on. The person can redirect you.

- **Simple.** One module, a small utility, a narrow question such as "how does function X work". Explore and explain in one pass yourself. Use the output sections in `references/explainer-prompt.md` and skip the explorer findings. Go to step 4.
- **Complex.** A subsystem across several files, packages or services, a feature that cuts across layers, or a full architectural overview. Go to step 2.

When in doubt, take the simple path.

## 2. Explore (complex only)

Split the question into 2 to 4 angles. Each angle is a distinct slice of the subsystem: for example the entry point and request flow, the storage layer, the error and retry paths, the configuration and wiring. Read `references/explorer-prompt.md` from this skill's folder.

**On the main chat**, spawn one sub-agent per angle with `spawn`. Build each task from the explorer template with its angle filled in. The template opens with "Do not edit files. Commit nothing." Keep that line: there is no read-only flag, so the brief is the only thing that keeps an explorer from writing. `spawn` refuses while `@` holds changes. If your uncommitted work is a finished unit, commit it first. If it is not, explore yourself as on a chat. Spawn all the explorers in one turn, then end the turn. Each explorer's report arrives in a later turn. When a report arrives and others are still running, note it and end the turn again. Synthesize only when every angle has reported or dropped out. Name any dropout in the answer.

The explorers fork your conversation, so they already know the question. The template still matters: it fixes the angle and the report shape.

**On a chat or a sub-agent**, `spawn` is refused. Explore yourself, one angle after another. For each angle, write its report in your notes in the explorer template's sections before you start the next one. Use `codemode` for bulk reads and greps, so only the result enters your context.

## 3. Synthesize (complex only)

Write the explanation yourself, following `references/explainer-prompt.md` with every explorer's findings as its input. Reconcile overlaps. Where two explorers disagree, read the code and settle it. Do not re-explore from scratch.

## 4. Present

Present the explanation. Keep the sections in the explainer template and drop any that do not apply.

A long explanation goes on the canvas: `canvas_write` with the explanation as Markdown, with Mermaid for diagrams. The reply then gives the overview in a few sentences and points to the canvas item. A short answer goes in the reply.

Load the `humanizer` skill and apply it to the prose before you send it.

## Output format

From `references/explainer-prompt.md`, dropping any section that does not apply: Overview, Key Concepts, How It Works, Where Things Live, Gotchas.

For "where should this live" questions, end How It Works with the placement answer: the module or package that should own it, the boundary it sits behind, and the existing code that already does the closest thing.
