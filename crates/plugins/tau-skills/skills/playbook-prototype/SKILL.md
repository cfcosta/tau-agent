---
name: playbook-prototype
description: "Load when asked to prototype, mock up, sketch, or try a layout, or when a \"which approach?\" fork can be settled by observing it instead of asking. Makes you build throwaway variants in a scratch path behind one switch, observe them, and hand the winner to playbook-feature."
---

# Prototype

You own the design decision, not the code. The prototype is a throwaway instrument. The real build follows `playbook-feature`.

This is the one playbook where the usual bars invert. Speed beats polish. Code quality does not matter. There is no planning step. The rigor goes into picking the right design cheaply. Propose variations the person did not ask for. Throw an approach away and try another.

Use it before you `ask` the person "which approach?". If the answer is a fact you can observe by running something (behaviour, timing, layout, output, whether an eval separates two options), it is not the person's question to answer. Sketch it and let the result decide. Keep `ask` for a product or preference call no experiment can settle.

## How to run it

Copy the steps below into your working notes as a numbered checklist and tick each one off as you finish it. A step you do not do stays in the list as `skip: <reason>`.

## Steps

1. Scope the decision. Write down the one question the prototype exists to answer: which layout, which interaction, which density, or for an empirical fork, which behaviour, timing, or approach. No decision means no prototype; load `playbook-feature` instead.
2. Gather references when the design space is open. Look for prior art in the repository and in any docs or search tools you have. Put a short moodboard on the canvas with `canvas_write`: the themes, layouts, or approaches worth trying. Choosing a direction by taste is the person's call, so `ask` them to pick, with concrete options and your recommendation first. Skip this step when the direction is already set.
3. Build it throwaway in a scratch path, apart from production source. Prefer a directory outside the repository. If the prototype must import the repository's code, put it under `prototypes/<task>/` in the repository, and remember that every file in the working copy is recorded, so it will show up in the next commit unless you commit other work with `vcs_commit {paths}`. For a visual decision, use the lightest stack that renders the idea (one static web page with no build step is often enough). For a behavioural or timing decision, write the smallest script that exercises the question. No production framework, no tests, no abstractions.
4. Put variants behind one switch. When you compare alternatives, build them in one prototype with a single switch between them: a button row, a keypress, a command-line flag, an environment variable. Label each variant. This is `principle-exhaust-the-design-space` made cheap; load it.
5. Observe each variant on the matching surface. The observation is the test here, not an assertion. For a visual decision, take a screenshot of each variant and drive its interaction. Use the repository's `verify-*` skill if it has one, or a headless compositor or headless browser, never the person's real display. Put the screenshots on the canvas with `canvas_show`, one caption per variant. For a behavioural or timing decision, log the timing, print the output, or watch the render. If you compare numbers, load `principle-explain-the-number` and run each side at least five times, interleaved, before you trust a difference.
6. Decide and hand off. Present the variants, the tradeoffs, and a recommendation. The output is the decision and the throwaway artifact, not shippable code. Hand the chosen direction to `playbook-feature` for the real build, or to the `architect` skill if the shape still needs design.
7. Clean up. If the prototype lives inside the repository and a commit picked it up, delete it in its own commit (`chore(prototype): remove <task> prototype`). If the person wants to keep it for reference, leave it committed and say where it is. A prototype inside the repository that you neither committed nor deleted counts as uncommitted work, and tau holds a run that stops with uncommitted work, so settle it before you end. On a chat, finish with `vcs_land` once everything is committed. On the main chat, commit and you are done.

## Rules that hold throughout

- Decide reversible things yourself and say in the reply what you assumed. On a sub-agent, you cannot `ask`; pick the direction yourself and report the assumption.
- On the main chat you may `spawn` one sub-agent per variant when the variants are large and independent. Commit first, give each its own path (`prototypes/<task>/a/`, `.../b/`) so they do not clash, then end the turn. Review what lands yourself. Small variants are faster to write yourself.

## Reply

Load the `humanizer` skill for the reply. Short declarative sentences, no em dashes. Every claim carries its evidence or a label in the same sentence: measured, inferred, or guess. Never hand the person a check you could run.

Content: the variants you explored. The evidence: screenshots on the canvas for a visual decision, the observed output or timing for a behavioural one. The tradeoffs. Your recommendation. The scratch path. Say plainly that the prototype is throwaway.
