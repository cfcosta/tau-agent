# 0032: The watcher notes what you missed

- Status: accepted. Amends [0017](0017-plugins-bring-their-ui.md): tau-ui
  declares one more point, `tau.run.composer.band`, a line between the
  transcript and the composer.
- Date: 2026-10-09

## Context

An agent works for minutes and the person reads a part of it. Some of
what went by matters to them: a cost they did not plan for, a command
that does not do what its name says, work the run is about to throw
away. The agent does not say it, because it is busy with the task, and
the person does not ask, because they do not know there is a question.

Claude Code's "You should know" shows that a second look can catch this:
every few steps a side request reads the conversation and, rarely, offers
one short note. What makes it worth having is that it stays quiet. A
note that is trivia, repeats itself or covers what the run already said
teaches the person to ignore it.

## Decision

### A plugin, `tau-watcher`, off until switched on

- A side request reads the run's whole transcript every 6th step and
  answers `learn: none`, or one note: a line of at most 240 characters, a
  tag (`You should know` for how something works, `Heads up` for this
  session's work), and an explanation of a title and 3 to 5 bullets. The
  request goes through `PluginCtx::ask`: outside the run's conversation,
  its usage charged to the run.
- It is off by default (a note costs a request), and switched on in its
  settings pane (0029). The pane also picks the model; the default is the
  chat's own.
- The prompt defaults to `learn: none`, and asks for a note only where a
  consequence exists (money, time, work thrown away, a wrong result, a
  decision in progress). It skips what the run covered or the person
  showed they know, trivia, and anything the model is not sure of.
- The reply is parsed strictly. One that does not read is dropped: the
  plugin records that it was, and shows nothing.

### Quiet by cadence, back-off and memory

- No check while a note waits for an answer, or after a note in the same
  run.
- Each time the person writes a message past a note they have not
  answered, the checks that follow thin out: none are skipped for the
  first 2 times, then 2^(n-3) due checks are, at most 16. Answering a
  note starts the count again.
- The prompt carries the last 50 note lines and the lines the person
  marked "knew this", so a topic does not come back. A model writes
  every note: nothing in the plugin decides what is worth saying.

### Shown twice, then once

- The note is a record of the plugin and an annotation in the transcript
  under the step that asked for it: an amber left edge, `Heads up · line`
  or `You should know · line`, and the actions **Learn more** (the
  stored explanation, inline, with no second request), **Knew this
  already**, **Chat about it** and a dismiss.
- While the note is new (unanswered, and the person has written past it
  fewer than 2 times) it is also a one-line band between the transcript
  and the composer, with the same actions. Once the person acted, only
  the annotation stays.
- Every action is a record the interface stores (`Request::Record`), so a
  stored run draws what the live one did. **Chat about it** also puts
  the note in the composer as quoted text from a side agent, with control
  characters stripped and `@` mentions and `ultra` keywords neutralised.

### One crate

The host half only builds a prompt and makes a request, so by 0030 it
stays in the plugin's crate, `tau-watcher`, as `WatcherHost` beside
`WatcherUi`, always built. The reply parser, the cadence and the
seen-list are plain functions of that crate.

### A point between transcript and composer

`tau.run.composer.band` draws every contribution, in order, in the
transcript's column above the composer, which stays in place. `tau.run.composer` replaces the composer; this does not.

## Consequences

- The check runs inside the step it follows: that step's request waits
  for the side request, a second or two every 6th step, and the
  annotation lands under it. A background request would land the note
  later, under a step that did not ask for it.
- A run with a note is a little dearer; the plugin's spend shows on the
  Plugins screen like any other's.
- Back-off counts messages sent past a note, which the plugin sees as a
  run that starts while a note waits. A phone and a computer count the
  same, since both fold the same records.
- The prompt is the product. Its examples and its default are tuned by
  reading real notes, not by code.
