# 0019: The agent asks the person, in the composer's place

- Status: accepted. Built: `tau-ask` (`crates/plugins/ask`) and the
  `COMPOSER` point in `tau-ui-plugin`, which `tau-ui` draws in place of
  the composer. Reference: [ask.md](../reference/ask.md).
- Date: 2026-10-01

## Context

An agent often reaches a choice only the person can make: a preference,
a trade-off, which of two plans to follow. Without a tool for it, the
model either guesses or ends its turn with a question in prose, which
the person answers in prose, and the run starts over from a new
message.

Claude Code's `AskUserQuestion` gives the model a structured way to
ask: one to four questions, two to four choices each, several picked or
one, a preview per choice, an answer of the person's own always open,
and a note the person can add to an answer. The answer comes back as
the tool's result, and the turn goes on.

Nothing in tau held a tool call open for a person. `vcs_land` ends the
turn and lets a card take the answer later
([0009](0009-child-runs-land-on-their-parent.md)); approvals in
tau-mcp and the constitution are settings, not open calls. And the
composer had no extension point: no plugin could put anything in its
place.

## Decision

### A plugin, `tau-ask`, whose call waits

- The `ask` tool takes `{ questions: [{ question, header, options:
[{ label, description, preview? }], multi_select }] }`, with the
  limits above. A question breaking them is refused with the reason,
  and nothing is shown.
- The call holds open. Its plugin's host half keeps the call's sender
  by run and call id; the panel's answer reaches it through the
  plugin's `act`, which checks that it answers what was asked. A
  cancelled run ends the wait, and a call whose future is dropped
  while it waits is closed as it goes.
- Only the model calls it (`Exposure::ModelOnly`): Codemode drops the
  calls a script left running when the script ends.
- The result is text the model reads (each question and its answer,
  each note under its answer, or that the person declined), with the
  structured reply in `details` and `structured`.
- Holding the call open, rather than ending the turn, keeps the run's
  context, cache and plan as they were: the answer is one tool result,
  not a new message.

### Records, so stored runs show what was asked

`tau-ask` publishes `asked`, `answered` and `closed` (one `Record`
enum, `#[serde(tag = "kind")]`, shared by the tool and the fold). A
run that goes on from history closes, in its `start`, the calls the
history left waiting: an app that quit while a question waited leaves
one.

### A point for the composer's place

`points::COMPOSER` (`Point<AtRun>`): the first contribution draws in
place of the composer under a run's transcript; with none, the
composer is back. tau-ui calls it in one place, where it draws the
composer. tau-ask contributes the panel while a live run has a call
waiting. Other plugins can use it for input a run needs more than a
message: an approval, a form.

### Notes, per question

A note belongs to a question's answer, not to a choice
(`Answer::note`), as `AskUserQuestion`'s `annotations` do. `n` opens
it; the model reads it under the answer.

## Consequences

- While a question waits, the composer is gone: the person cannot
  steer the run until they answer or decline. The run's Cancel stays
  in its header on a computer; on a phone, Decline is the way out.
- Contributions draw without the window, so the panel takes the focus
  through a zero-size canvas as it first appears, and nothing gives
  the composer the focus back when the panel goes. A key typed in the
  composer just as the panel appears goes to the panel.
- Sub-agents get no `ask`: nobody watches them.
- A plugin's reports reach the interface with the run's next event, and
  no event comes while a call waits. The tool sends one update after
  publishing its question, which carries the report out. A loop that
  sent reports as they come would make this unneeded.
- A stored run whose question was never answered shows the card as
  "not answered", and no panel: only a live run's call can be answered.
