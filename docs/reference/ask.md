# tau-ask

The agent asks the person one to four questions and waits for the
answers. Crate: `crates/plugins/ask`. Decision:
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

- **Limits** (`Ask::check`): 1 to 4 questions; a header of 1 to 12
  characters; 2 to 4 choices with labels that differ; no question
  repeated. No choice may be called "Other": the person can always
  write their own answer. Choices of a `multi_select` question have no
  preview. A question that breaks a limit is refused, and the model
  reads why.
- **Recommending:** the recommended choice goes first, its label ending
  in "(Recommended)". The panel marks it.
- **The result** the model reads:

  ```text
  The person answered:
  "How should the ask tool wait for your answer?" = "Hold the call open (Recommended)"
    note: Cancel must drop the sender too.
  "Where else should a waiting question show?" = "Run list badge, Parent run, the tray icon"
  ```

  A checklist's answer is its labels in the question's order, then the
  person's own answer, joined by commas. Declining reads "The person
  declined to answer…". The reply as JSON (`Reply`) is in the result's
  `details` and `structured`.

- **Waiting:** the call holds until the person answers or declines, or
  the run is cancelled (an error: "The run was cancelled before the
  person answered."). However the call's future ends, its wait is
  forgotten, and a call that ended without saying how is closed: the
  `closed` report goes out at once, and its record is stored in the
  background.
- **Who calls it:** the model only (`Exposure::ModelOnly`): a script
  that ends drops the calls it made, with the person still answering.
  Sub-agents do not get the tool: nobody watches them.

## Records

`Record`, `#[serde(tag = "kind")]`:

- `asked { call, ask }`: the call waits.
- `answered { call, reply }`: `reply` is `{ "outcome": "answered",
"answers": [{ picked, other?, note? }] }` or `{ "outcome":
"declined" }`.
- `closed { call }`: it stopped waiting without a reply; a cancelled
  run, or, in a run's `start`, a call its history left waiting.

## The panel

While a live run has a call waiting, the panel draws in the composer's
place (`points::COMPOSER`) and takes the keys:

| Key   | On a question                                                       | On the review                         |
| ----- | ------------------------------------------------------------------- | ------------------------------------- |
| ↑ ↓   | move between rows                                                   |                                       |
| 1–5   | pick that row (one answer: and go on); the last is the person's own |                                       |
| Enter | pick the row (one answer: and go on); a checklist goes on           | send, once every question is answered |
| Space | toggle the row                                                      |                                       |
| ← →   | the question before or after; after the last, the review            | back                                  |
| n     | open the question's note                                            |                                       |

- **The person's own answer** is the last row: picking it, or typing in
  its field, makes it the answer (alongside the choices, in a
  checklist). Enter in the field takes it; Esc goes back to the rows.
- **Notes** belong to a question's answer. In the note, Enter saves,
  Shift+Enter starts a line, and Esc closes; the text stays either way.
  A tab with a note shows a pencil, and a saved note shows under the
  rows, where a click opens it again.
- **Previews:** when a one-answer question's choices have previews,
  the rows sit beside the preview of the row in focus.
- **Review:** each header, its answer and its note; a click goes back
  to that question. Send answers waits until all are answered;
  Decline to answer is always there.
- **Sending:** once the answers go, the panel says so and takes nothing
  more. If the host refuses them (the call stopped waiting), it says
  why and takes answers again.
- **Several runs:** each waiting call keeps its own draft, by its run
  and its id, so the person can go between runs and come back to what
  they wrote. A panel takes the keys once, as it first appears, and
  not while the person is writing in the composer; once answered, the
  composer has them again.
- **On a phone** the panel's foot has Cancel run, which the run's
  header has on a computer.
- The card of an `ask` call shows the headers, and each answer with
  its note, from the call's own arguments and result.

## Code

- `ask.rs`: `Ask`, `Reply` and their checks and text.
- `host.rs` (feature `host`, on by default): `Waiting`, the tool and
  the agent plugin.
- `ui/`: the fold (`State`), the draft and its keys (`Draft`), the
  panel, and the card.
