---
name: reflect
description: "Load when the person says \"reflect\", or at the end of a long task to capture what it taught. Reviews this conversation through three lenses, filters the findings, and turns each kept one into an approved edit to the repository's skills, its instructions file, or memory."
---

# Reflect

Mine the current conversation for durable lessons, then turn each one into a concrete edit the person approves.

Skip it when the conversation was trivial, off-topic, or already covered by a skill you followed correctly. A one-off is not a lesson.

## 1. Review through three lenses

Each lens has a brief in this skill's folder. Read the three files first.

| Lens | Looks for | Brief |
|---|---|---|
| Judgment | the durable principle behind a specific incident | `references/judgment-reviewer.md` |
| Tooling | the command, flag, path, or quirk the next agent would re-derive, and context the person handed over that you could have fetched | `references/tooling-reviewer.md` |
| Divergent | second-order effects, things that worked for the wrong reasons, what did not happen but should have | `references/divergent-reviewer.md` |

**On the main chat**, spawn one sub-agent per lens. Commit any pending work first, because `spawn` refuses while `@` holds changes. Pass each brief verbatim as the `task`. A sub-agent forks this conversation, so it already has the whole transcript: there is nothing to locate or attach. The forking also means the reviewers share your view of what happened. They are not independent. Their diversity comes from the lens, so keep the briefs distinct and do not add your own conclusions to them. Spawn all three in one turn, then end the turn. Each reviewer's report starts a turn of yours. Note it and end the turn again until all three are in.

**On a chat or a sub-agent**, where `spawn` is refused, run the three lenses yourself, one after another. Read each brief as your own instructions. Where it says "the conversation you forked from", it means this conversation. Finish one lens and write its numbered findings in your working notes before you start the next, so one lens does not bleed into the others.

The reviewers do not edit files. The briefs say so.

## 2. Synthesize

Do the synthesis yourself, following `references/synthesizer.md`, with the three reviewers' findings as its input. Before you accept a row that edits a skill or the instructions file, read the target, so you can reject what it already says. The synthesis returns three lists: Accepted, Rejected, and Backlog.

## 3. Move enforceable rules to the backlog

Move every Accepted item that a lint, a script, a type, a config flag, or a runtime check would enforce more reliably to Backlog. Prose is for what a mechanism cannot enforce. Load `principle-encode-lessons-in-structure`.

## 4. Ask before applying

Put the synthesis on the canvas (`canvas_write`, one item) so the person can read the full tables.

Then ask with `ask`, one question per Accepted row, up to four questions per call. Each question quotes the problem and the proposal in one line each. The options are "Keep (Recommended)", "Change", and "Drop". If the person picks "Change", rewrite the proposal from their note and ask about the new wording, or, with no note, offer two concrete rewordings.

Offer each Backlog item the same way, with the options "Build the check now (Recommended)", "List it as unenforced", and "Drop". "Build the check now" means load the `correct` skill for that one class. "List it as unenforced" means add a row to the rule table in the repository's instructions file with "no enforcer yet", so a later `correct` run finds it.

Do not apply anything the person has not kept. A skill or instructions edit changes every future run in this repository.

## 5. Apply

Route each kept row by its target:

- **A skill in this repository** (its own skills folder, for example a `verify-<app>` skill): edit its `SKILL.md` or its references. A `tune description` row rewrites the `description` line so it states when to load the skill first. A `new skill` row writes a new folder with a `SKILL.md` whose frontmatter has only `name` and `description`, in the same place as the repository's other skills.
- **The repository's instructions file**: add or tighten the rule in the section it belongs to. Keep its rule table current.
- **Memory**: a durable fact that holds across repositories (a preference of the person's, a tool quirk) goes to `memory_write`. Search with `memory_search` first and update the existing note instead of adding a duplicate. Memory is not for task progress or TODOs.
- **A skill outside the repository** (one of tau's built-in skills, or one in the person's own skills folder): do not edit it. Give the exact proposed text in your summary so the person can apply it.

Commit each applied repository edit with `vcs_commit`, one commit per row, with a message such as `docs(agents): say that migrations run before seeding`. On a chat, finish with `vcs_land`.

## 6. Summarize

A short list, no preamble:

- Applied: the file or memory note, and what changed, one line each.
- Proposed outside the repository: the skill, and the exact text, one line each.
- Backlog: what was built, listed as unenforced, or dropped, one line each.
- Dropped: one line per rejected finding, with the synthesizer's reason.
