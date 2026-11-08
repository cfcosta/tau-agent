# tau-ask

The agent asks the person one to four questions and waits for the
answers. It is a Luau plugin tau comes with (ADR 0050):
`plugins/tau-ask`, its code in `src/plugin.luau`,
`src/ask.luau` and `src/panel.luau`. Decision:
[0019](../decisions/0019-ask-the-person.md).

## The tool

`ask` takes:

```json
{
  "questions": [
    {
      "question": "How should the ask tool wait for your answer?",
      "header": "Waiting",
      "options": [
        {
          "label": "Hold the call open (Recommended)",
          "description": "The answer comes back as the result.",
          "preview": "let (tx, rx) = oneshot::channel();"
        },
        { "label": "End the turn", "description": "Read it as the next message." }
      ],
      "multi_select": false
    }
  ]
}
```

- **Limits** (`ask.check`): 1 to 4 questions; a header of 1 to 12
  characters; 2 to 4 choices with labels that differ; no question
  repeated. No choice may be called "Other": the person can always
  write their own answer. Choices of a `multi_select` question have no
  preview. A question that breaks a limit is refused, and the model
  reads why; text is compared trimmed of Unicode's white space, and a
  header that is too long is quoted as the model wrote it, escapes and
  all.
- **Recommending:** the recommended choice goes first, its label ending
  in "(Recommended)".
- **The result** the model reads:

  ```text
  The person answered:
  "How should the ask tool wait for your answer?" = "Hold the call open (Recommended)"
    note: Cancel must drop the sender too.
  "Where else should a waiting question show?" = "Run list badge, Parent run, the tray icon"
  ```

  A checklist's answer is its labels in the question's order, then the
  person's own answer, joined by commas. Declining reads "The person
  declined to answer…". The answers as JSON (`{ call, answers }` or
  `{ call, declined }`) are in the result's `structured`.

- **Waiting:** the call answers `tau.ask` with the questions, each
  taking the person's own answer, and holds until the person answers
  or declines, or the run is cancelled (an error: "The run was
  cancelled before the person answered."). The host checks the answers
  against what was asked, and the plugin's `answered` action takes them
  in the run and writes what the model reads.
- **Who calls it:** the model only (`model_only`): a script that ends
  drops the calls it made, with the person still answering. Sub-agents
  declare the tool, so their tools match their caller's and read its
  prompt cache (ADR 0022), but nobody watches them: a call fails with
  "A sub-agent has no one to ask: decide yourself, and say in your
  answer what you assumed and why." An agent's runs do not have it:
  they ask through tau-agents' own `ask` (ADR 0037).

## State

The run's `ctx.state.calls`, in the order they asked: each `{ call,
ask, draft, focus, reply?, closed? }`.

- `draft`: the question in view (`tab`; one past the last is the
  review), each question's row in focus, the rows picked, what the
  person wrote and whether it counts, each note, and whether the note
  in view is open. Rows and tabs count from 0.
- `focus`: `{ panel, other, note }`, raised to give the panel, the
  field of the person's own answer or the note's field the keys once.
- `reply`: `{ answers }` or `{ declined = true }`, once answered.
- `closed`: it stopped waiting without a reply: the run ended
  (`run_end`, a cancelled run too), or a run went on from a history
  that left it waiting (`start`).

## The panel

While a live run has a call waiting, the panel draws in the composer's
place (the view's `composer`) and takes the keys:

| Key   | On a question                                                       | On the review                         |
| ----- | ------------------------------------------------------------------- | ------------------------------------- |
| ↑ ↓   | move between rows                                                   |                                       |
| 1–9   | pick that row (one answer: and go on); the last is the person's own |                                       |
| Enter | pick the row (one answer: and go on); a checklist goes on           | send, once every question is answered |
| Space | toggle the row                                                      |                                       |
| ← →   | the question before or after; after the last, the review            | back                                  |
| n     | open the question's note                                            |                                       |

Each key and click is one of the plugin's actions on the run's state,
so the draft is the same on every device, and a phone may answer what
the computer started.

- **The person's own answer** is the last row: picking it puts the keys
  in its field, and Enter there takes what it holds as the answer
  (alongside the choices, in a checklist); Esc goes back to the rows.
- **Notes** belong to a question's answer. Enter in the note keeps it,
  and Esc closes it. A tab with a note shows a pencil, and a kept note
  shows under the rows.
- **Previews:** when a one-answer question's choices have previews,
  the rows sit beside the preview of the row in focus.
- **Review:** each header and its answer; Edit goes back to that
  question. Send answers waits until all are answered; Decline to
  answer and Cancel run are always there.
- **Sending** leaves the state as it is: the run takes the answers, and
  the panel goes once the call has them. Answers the host refuses (the
  call stopped waiting, or another device answered first) are the
  action's error.
- **Several runs:** each waiting call keeps its own draft, in its run.
  A panel takes the keys once, as it first appears; once answered, the
  composer has them again.
- **The card** of an `ask` call shows the headers, and each answer with
  its note; while it waits, "Answer in the panel below."
- **Elsewhere:** while a call waits, the view's `asks` is its first
  question, and its status says how many questions wait. The run's row
  in the sidebar and on a phone says "Asks you a question", and a
  desktop notification shows the question while tau's window is not
  focused ([attention.md](attention.md)).

## Code

- `src/ask.luau`: the limits and their check, whether answers fit,
  the text the model reads, and the draft and its keys.
- `src/escaped.luau`: the characters a quoted header writes as
  `\u{...}`.
- `src/panel.luau`: the panel and the card.
- `src/plugin.luau`: the tool, the actions, `start`, `run_end` and the
  view.
- `tests/`: the cases and properties; `tests/luau.rs` runs them
  as tau loads the plugin, and checks the demo's fixtures.
