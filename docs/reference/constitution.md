# tau-constitution

Checks a run's tool calls and final answer against rules, asking Jev
(TypeSafe's System One model) how likely each rule is broken. Crate:
`crates/plugins/constitution`. Design: [plugins.md](plugins.md).

## Rules

A constitution is a list of rules and two settings. Each repository's is
kept in the plugin's own SQLite file,
`<tau's directory>/plugins/tau-constitution/constitution.db`
(`constitutions` and `constitution_rules`, with the plugin's own
migrations and sqlx metadata in `crates/plugins/constitution`), and
edited only through tau's UI. The same file keeps the flagged calls and
answers a person reviewed (`reviewed`).

- **A rule** has an id (`R1`, `R2`… given when it is added), its text,
  where it applies (`on`), and two violation probabilities: `review`
  flags for a person, and `block` refuses the call. Review is at most
  block, and both are between 0 and 1.
- **`on`** names where a rule applies: `tool.field` (every value under
  that key in the call's arguments, however deep, so `edit.newText`
  covers each edit) or `final answer`.
- **`on_error`** decides what happens when Jev cannot answer: `allow`
  (the default) or `block`.
- **`max_holds`** is how many times one run's final answer may be sent
  back (3 by default).
- Rules are checked when added or edited, and again when read back from
  the store. A stored constitution that does not check fails the run at
  start, saying why: broken rules are never silently ignored. None
  stored is no rules.
- The plugin reads the rules at every check, through a `Live` handle the
  host replaces when they are edited, so an edit applies from the next
  tool call, in runs already going too. `ConstitutionPlugin::new` takes
  rules that never change, for tests and embedders.

## What happens

| Where        | Violation probability       | Result                                                                                                                              |
| ------------ | --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Tool call    | ≥ `block`                   | Refused. The model gets the rule, quoted, and fixes the call.                                                                       |
| Tool call    | ≥ `review`                  | Runs, flagged for review.                                                                                                           |
| Final answer | ≥ `block`                   | Sent back with the rule, up to `max_holds` times.                                                                                   |
| Final answer | ≥ `review`, or past the cap | Stands, flagged.                                                                                                                    |
| Jev fails    | —                           | `on_error`: `allow` lets the call run or the answer stand; `block` refuses the call, or sends the answer back while holds are left. |

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
the run, so history can show it again. A failed check is reported and
recorded too, as `{"kind": "error", ...}` with the call and tool (none
for the final answer), the message, `on_error`, and for a final answer
whether it was sent back (`held`).

## In tau-ui

- Each repository has its own constitution, kept in tau's store under
  the repository's checkout path, and edited only on the Constitution
  screen. Nothing is written to the repository or to a file, and an edit
  applies from the next tool call, in runs already going too.
- The Constitution screen:
  - **Rules:** each rule's places, strictness and what it did in the
    repository's runs: the runs loaded in the UI as they go, and every
    other stored run from the plugin's records
    (`Store::plugin_entries_everywhere`), counted the same way
    (`ConstitutionStats::add`).
  - **Editor:** writes and edits a rule, with places picked from a list
    (or any `tool.field`), Lenient / Balanced / Strict thresholds
    (flag/block at 0.5/0.9, 0.3/0.8, 0.2/0.6) or steps of 0.05, and
    **Try it**. Try it asks Jev about the repository's latest calls and
    answers the rule reads, with the same question and state a check
    uses (`tau_constitution::try_rule`), so nothing runs again.
  - **Review:** flagged calls and answers, with the rule and the score
    against its thresholds; Looks fine takes one off the queue. Next to
    it, what the rules handled on their own.
  - **Settings:** what happens when Jev cannot answer (Let through, or
    Refuse: `on_error`), and how many times one run's answer may be
    sent back (`max_holds`, 0 to 10). Saved like the rules, and read at
    the next check.
  - **Rules that cannot be read** from the store: why, and that runs in
    the repository fail at start until they can (once there is a
    TypeSafe key; without one nothing is checked). No edit can fix
    them, so the banner offers to remove them, once confirmed
    (`Host::reset_rules`), to start again.
- Checks need a TypeSafe key, added on the Models screen and kept in
  tau's config directory (`typesafe-key`), readable only by the user.
  Without one, runs are not checked and the screen says so.
- Blocked and flagged calls show on their cards in the transcript, live
  and in history, and every call checked shows each rule's score; a
  held answer shows as a note with its count (`continuation 1 / 3`).
- The inspector's plugin list says what the plugin did in the run
  (`1 blocked · 1 flagged`), with the run's checks under it
  (calls and answers checked, questions asked, blocks, flags and holds
  with their rules, Jev's cost), and a finished run's outcome lists its
  continuations and offers to review a flagged call.
