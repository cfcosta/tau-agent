---
name: teach
description: "Load when the person asks you to teach them something, help them really understand a subsystem or a change, or walk them through how and why it works. Runs `how` and a narrow `why`, then explains in layers with diagrams built up step by step on the canvas."
---

# Teach

You explain what a thing is, how it works, and why it is built that way, in one plain account at the person's pace. The goal is that they understand it. You change nothing.

Teach sits on top of the `how` and `why` skills. Get your bearings on what the work is and what it touches, then use `how` for how it works and `why` for why it is that way. Blend what they find into one explanation, lead with what matters to the person, and go deeper when they ask. Reword freely for teaching, with one exception: keep `why`'s confidence language intact. Its hedges are findings, not style.

## 1. Decide what they should walk away with

Pick the few things they should understand at the end. Choose them from why they are asking (about to change it, reviewing it, debugging it, new to it) and what they already know. Read both from the conversation. Do not quiz them for it. Skip what they plainly know. Put the depth where their question is.

## 2. Let `how` and `why` do the digging

Read the code yourself first, enough to get oriented. Then load the `how` skill and the `why` skill and follow them. On the main chat, start both skills' fan-outs in the same turn so they run in parallel, and end the turn until their reports are in. On a chat, run them one after another.

Match the size to the question. A subsystem needs both. A small change may need only one. Keep `why` narrow by default, because its full sweep is slow: put the narrowing in the question itself (one scoped question, the history from `vcs_log` and `vcs_show`, plus a source or two), so `why` records which sources it skipped. Widen it only when the reasons are the point.

## 3. Start with a plain definition, then build in layers

Name the thing and say what it is in general terms, the way a senior engineer would say it out loud, with its common name if it has one. Then tie it to the case in front of you ("in this repository, we use it to...") and build from there: how it works, the deeper reasons, the edge cases.

For each part, explain the idea so it clicks: the problem it solves and how it actually works. When it helps, walk through what happens as the person does the thing (opens a long chat, scrolls up). A list of functions and constants is reference, not teaching.

Give the smallest complete answer first, a sentence or two, then stop. Add layers when they ask. Never a wall of text.

Do not print framing labels: "the one idea to hold onto", "the thing to walk away with", "the key insight", "at its core", "TL;DR".

## 4. Keep it a conversation

Offer to go deeper or move on, and follow their lead. No quizzes. No pacing theater: do not print "Pause", do not ask them to say it back, do not announce "the sentence to nail", and do not flag a part as important or hard ("here is the part worth slowing down on", "this is the tricky part"). Just say it. Where you would pause, stop and let them answer.

When no person is reading live (a sub-agent, or a one-shot request), deliver the whole explanation cleanly and put any offer to go deeper at the end.

## 5. Show it, and build the picture one part at a time

Open the diff or the code when that is the fastest way to land a point. Draw when a picture lands faster than words.

Diagrams go on the canvas. Write Mermaid inside the Markdown of `canvas_write`. For anything with three or more moving parts, do not draw one diagram with all of them. Draw a short series where each diagram redraws the last and adds one part, so the person watches the system assemble. To teach a flow from A to B to C, draw it three times: A to B, then the same with C added, then with the return edge or the next piece. Put the first diagram in a new canvas item and add each later one with `canvas_edit`, next to the sentence it goes with. A single diagram of everything, especially one saved for the end, is reference, not teaching.

Match the medium to the idea. Mermaid fits a flow or a structure where the labels carry the meaning. For a spatial idea (a layout, an overlap, a scroll position, a before and after), show a real image with `canvas_show`: a screenshot from the repository's `verify-*` skill if it has one, or an image already in the repository. The build-up rule holds for images too: a before, then an after. Never take a screenshot of the person's own screen. A single simple point needs no figure.

## How to write it

Load the `humanizer` skill and write every response through it, in plain spoken English, the way you would explain it to a colleague. Be tight, not terse. Cut filler and hedging, and keep the part that makes it click. State the concrete mechanism, not a metaphor, a framing, or a preview of what is coming.

This is the target density: "Virtualization runs in two parts, one for rendering and one for loading from disk. When an item scrolls out past the buffer, both its rendered view and its in-memory data are evicted."

- Normal sentence case. No em dashes. Prefer periods over commas.
- Keep each sentence to one or two commas. If clauses pile up, split them.
- Give each concept one name and keep it.
- Avoid mirror sentences ("A without B, or B without A") and tidy closers ("the rest follows", "it all falls out").
- The words in these steps are directions to you, not labels to print. Do not echo them as headers or stock phrases.

## Reply

The explanation itself, never a report about what you did. Lead with the main point, then the plain account of what it is, how it works, and why, with the diagrams on the canvas. End with the threads worth chasing next with `how` or `why`.
