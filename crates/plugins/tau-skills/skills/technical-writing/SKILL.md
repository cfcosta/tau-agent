---
name: technical-writing
description: "Load before writing or reviewing a commit message, a change description, a README, an ADR, or any doc. Picks one doc mode, then applies sentence rules (Google developer style, simplified technical English, global English) and Conventional Commits."
---

# Technical writing

Write so a tired engineer understands on the first read. Apply every layer below that fits the text.

Three rules sit above the layers:

- Cut every word that does no work. If the sentence survives without a word, the word goes. "In order to" is "to". "It is important to note that" is nothing.
- Use the short, everyday word. "Use", not "utilize". "Help", not "facilitate". "Do", not "perform". A long word has to buy its length with precision.
- When a rule makes a sentence worse, fix the sentence another way or leave it alone. A sentence that follows every rule and sounds like a machine wrote it has failed.

The codebase is the word list. Write the real symbol, file, flag, or command name, not a synonym or a description of it.

Do not invent jargon. Use the words a developer would say out loud: "move", "delete", "a budget that only decreases", not "evacuate", "ratchet", or "endgame". A named pattern is fine when the doc says what it means the first time. If you find a new offender, propose it and its plain replacement for the `humanizer` skill's abstract-metaphor pattern (number 25) in your reply. Do not edit that skill.

Load the `humanizer` skill and apply it to every text this skill touches. It owns the catalogue of AI tells: vocabulary, filler, hedging, formatting.

## Vary the rhythm

A doc can obey every layer and still read machine-written: every sentence clipped short, no view anywhere, nothing specific.

- Mix sentence lengths on purpose. Short sentences land a point. Longer ones carry a fact with its condition or consequence.
- One thought per sentence does not mean one length per sentence. Split the sentence that carries two thoughts. Keep the long sentence that carries one.
- Have a view where the mode allows it. An explanation weighs trade-offs, so say what you make of them instead of listing pros and cons. Reference stays dry.
- Be specific, not sterile. Not "schema changes can cause issues" but "a column rename fails the build".

## Pick the mode first (Diátaxis)

One document, one mode. Two questions pick it. Does the content inform action (doing) or understanding (thinking)? Does it serve learning or work?

| | Learning | Work |
|---|---|---|
| Action | Tutorial | How-to guide |
| Understanding | Explanation | Reference |

- **Tutorial.** The learner's success is your job. Open with what they will build. Every step produces a visible result, and you tell them what they should see. Explanation shrinks to one clause and a link. Write as "we", in commands.
- **How-to guide.** Steps to a goal a competent person already has. Action only, no background, forks allowed ("If you want x, do y"). Name it by the task: "How to rotate the signing key", not "Signing keys".
- **Reference.** Facts for lookup. Describe only: no instruction, no opinion, no hedging. Mirror the structure of the thing described. Generate it from code where you can.
- **Explanation.** One bounded topic and its why: design decisions, history, constraints, alternatives. Opinion is allowed here and nowhere else.

Do not mix modes. No reference tables inside a tutorial, no hand-holding inside reference, no arguing inside a how-to. Split and link instead. If the run has a `diataxis` skill, it holds the full rules and a mode-bleed check; load it for a large doc or a doc review.

## Write sentences to the reader (Google developer style)

- Talk to the reader as "you", in the present tense. Use "will" only for things that happen later.
- Say who does what: "the compiler checks", not "is checked". Passive is fine only when the actor is unknown or beside the point.
- Write instructions as commands: "Run `./scripts/check`." State facts plainly. Never "should be done".
- Put the condition before the instruction: "To delete the cache, run `tool clean`."
- Put the common case first and the exceptions after.
- Sound like a knowledgeable colleague. No buzzwords, no figurative language, no "please" in instructions, and never "simply", "easy", or "quickly" in a procedure. If it were simple, the reader would not be here.
- Do not pre-announce ("we will soon support...") and do not start consecutive sentences with the same phrase.
- Link with words that say where the link goes. Never "click here".
- Headings carry the point, not just the topic ("Pick the mode first", not "Modes"). Sentence case. One h1 per page, no skipped levels.
- Numbered lists for sequences, bullets for everything else. Introduce a list with a complete sentence. Keep the items parallel.
- Code, paths, and commands go in code font. Use serial commas. Drop "etc." and say up front that a list is partial.

## Make statements load one at a time (simplified technical English)

- One instruction per sentence. One thought per sentence everywhere else.
- Split instructions longer than about 20 words and other sentences longer than about 25.
- Put the warning or condition before the step it guards.
- Keep "the" and "a". "Remove backup file" reads two ways. "Remove the backup file" reads one.
- Give each word one meaning and keep it. Pick one word per action: "start" everywhere, not "start" here and "initiate" there.
- Write procedures as direct commands, never as narration and never in the passive.
- Avoid "-ing" words where you can.

These are the core of ASD-STE100. If the run has an `asd-ste100` skill, it holds the full standard; load it when the person asks for STE or for a procedure manual.

## Leave no sentence open to two readings (global English)

- Keep "only" and "not" next to the word they change: "only fails on growth" and "fails only on growth" say different things.
- Break up long noun strings: "the proto import budget check script" becomes "the script that checks the proto-import budget".
- Make every "it", "they", and "this" point at one obvious thing. Repeat the noun when in doubt. Never use "this" or "which" to point at a whole clause.
- Do not drop verbs. "Phase 1 moves the converters and Phase 2 the runtime" leaves Phase 2 without one.
- Keep the small words that show structure, such as "that" in "Check that the switch is off".
- Repeat the article in a series when it prevents a misread: "the client and the host", when they are two things.
- Say which parts "and" or "or" joins when a sentence can group two ways. "Both...and", "either...or", and "if...then" disambiguate for free.
- Use periods, not semicolons. Replace an em dash with a new sentence.
- Make text in parentheses a full grammatical unit, or its own sentence. Never form plurals with "(s)".
- No slashes: write "a, b, or both" instead of "a/b" or "and/or".
- Call each thing by one name everywhere. A doc that says "the gate", "the ratchet", and "the budget check" for one thing teaches three things. Do not rename what did not change.
- Skip idioms, Latin abbreviations, and metaphors.

## Commit messages

Commits are how the person reviews your work, so the message is a briefing. tau's `vcs_commit` takes a Conventional Commits message.

- Subject: `type(scope): summary`. Types: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `build`, `ci`, `chore`, `revert`. The scope is the package or module the change lives in.
- Read a few recent subjects with `vcs_log` and match the repository's own style (case, mood, scope names). Its instructions win over this list.
- Keep the subject under about 72 characters, with no trailing period. Say what changed in behavior, not which files you touched.
- Mark a breaking change with `!` after the scope and a `BREAKING CHANGE:` footer that says what callers must do.
- The body, when one helps, says why the change was needed, what it does that the subject could not fit, and how you proved it (the command you ran, the test that now fails without the fix). Wrap it at about 72 columns.
- One logical change per commit. If the subject needs "and", it is probably two commits.
- Never paste logs, hash lists, or metric tables into a message. Give the one number that matters and say where the rest lives.

A change description for a chat's landed stack or for the person's pull request follows the same rules: a briefing a reviewer reads in under a minute. Lead with what changed and why, then how it was verified, then what to look at closely.

## READMEs and ADRs

- A README is a landing page. Say in one paragraph what the thing is and who it is for. Then give the shortest path to a working result (a how-to). Then link to reference and explanation instead of inlining them. Write real paths. Make every count or tree claim true at the commit that lands it, and include the command that regenerates it.
- An ADR is an explanation. Give it a number, a title that states the decision, a date, and a status (proposed, accepted, superseded by N). Then: the context that forced a choice, the decision, the alternatives you rejected and why, and the consequences, including the bad ones. Never rewrite an accepted ADR. Write a new one that supersedes it. Follow the repository's existing ADR folder and template if it has one.

Product UI strings are not documentation. Use the product's copy rules for those.

## Worked example

Before:

> Configuration of the proto import ratchet budget script parameters is performed via budget.json. Note that it's important to remember that running with --write, which updates the committed budget to reflect the current count, should only be done when lowering it. If exceeded, CI fails.

After:

> `scripts/budget` reads the committed budget from `budget.json` and counts the files that import protos. If the count exceeds the budget, CI fails. Run `scripts/budget --write` only to lower the budget.
