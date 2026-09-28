# tau-constitution

Checks a run's tool calls and final answer against rules, asking Jev
(TypeSafe's System One model) how likely each rule is broken. Crate:
`crates/plugins/constitution`. Design: [plugins.md](plugins.md).

## Rules

A constitution is TOML:

```toml
# What to do when Jev cannot answer: "allow" (the default) or "block".
on_error = "allow"
# How many times one run's final answer may be sent back.
max_holds = 3

[[rule]]
id = "R2"
text = "Library code returns errors. No unwrap or expect outside tests."
on = ["edit.newText", "write.content"]
review = 0.3   # flag for a person at this violation probability
block = 0.8    # refuse the call at this one

[[rule]]
id = "R6"
text = "The final answer names the tests that ran and their result."
on = ["final answer"]
```

- `on` names where a rule applies: `tool.field` (every value under
  that key in the call's arguments, however deep, so `edit.newText`
  covers each edit) or `final answer`.
- `review` defaults to 0.5 and `block` to 0.8. Review is at most block.
- A file that does not parse fails the run at start, saying why: a
  broken constitution is never silently ignored. No file is no rules.

## What happens

| Where        | Violation probability       | Result                                                        |
| ------------ | --------------------------- | ------------------------------------------------------------- |
| Tool call    | ≥ `block`                   | Refused. The model gets the rule, quoted, and fixes the call. |
| Tool call    | ≥ `review`                  | Runs, flagged for review.                                     |
| Final answer | ≥ `block`                   | Sent back with the rule, up to `max_holds` times.             |
| Final answer | ≥ `review`, or past the cap | Stands, flagged.                                              |
| Jev fails    | —                           | `on_error`: runs (reported), or refused.                      |

Jev sees only the fields a rule names, as the model wrote them: never
tool output or file content, which could try to steer it. All rules on
one call go in one request; its cost is charged to the run.

## Reports

Every check is a `Check` (`kind`: `checked`): the call and tool (none
for the final answer), each rule asked about with its score, whatever
it decided, and what Jev cost. Every decision is then a `Verdict`
(`kind`: `blocked`, `flagged`, `held`; the rule, its text, the score,
the call and tool, the reason given, and for a hold which one it is
out of `max_holds`).
The plugin reports it (`RunEvent::PluginReport`, before the event it
explains, such as the refused call's `ToolEnd`) and records it with
the run, so history can show it again. A failed check is reported as
`{"kind": "error", ...}`.

## In tau-ui

- Each repository has its own constitution, kept in tau's directory for
  the repository (`repos/<name>-<hash>/constitution.toml`), so rules
  apply to the next run without a commit. The Constitution screen lists
  them, adds and removes them, and shows the review queue.
- Checks need a TypeSafe key, added on the Models screen and kept in
  tau's config directory (`typesafe-key`), readable only by the user.
  Without one, runs are not checked and the screen says so.
- Blocked and flagged calls show on their cards in the transcript, live
  and in history, and every call checked shows each rule's score; a
  held answer shows as a note with its count (`continuation 1 / 3`).
- The inspector's plugin list says what the plugin did in the run
  (`1 blocked · 1 flagged`), its Plugins tab adds the run's checks
  (calls and answers checked, questions asked, blocks, flags and holds
  with their rules, Jev's cost), and a finished run's outcome lists its
  continuations and offers to review a flagged call.
- The Constitution screen's Edit link opens the file in the user's
  editor, making it first if there is none.
