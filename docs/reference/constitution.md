# tau-constitution

Checks a run's tool calls and final answer against rules, asking Jev
(TypeSafe's System One model) how likely each rule is broken. It is a
Luau plugin tau comes with (ADR 0050): its code is the Luau in
`plugins/tau-constitution/` (`src/plugin.luau`, and
`src/rules.luau`, `src/checks.luau`, `src/page.luau`), which
tau-luau-plugins loads as always active. Design: [plugins.md](plugins.md).

## Rules

A constitution is a list of rules and two settings. Each repository's is
kept in the plugin's store in that repository (`uses.store`), under
`constitution`, and edited only on the plugin's page. The same store keeps
what the checks did in each run, what waits for a person, what the rules
handled, and the latest calls and answers to try a rule on.

- **A rule** has an id (`R1`, `R2`… given when it is added, the lowest
  free), its text, where it applies (`on`), and two violation
  probabilities: `review` flags for a person, and `block` refuses the
  call. Review is at most block, and both are between 0 and 1.
- **`on`** names where a rule applies: `tool.field` (every value under
  that key in the call's arguments, however deep, so `edit.newText`
  covers each edit) or `final answer`, in any case. Text is trimmed of
  white space as Unicode has it.
- **`on_error`** decides what happens when Jev cannot answer: `allow`
  (the default) or `block`.
- **`max_holds`** is how many times one run's final answer may be sent
  back (3 by default, up to 10 on the page). The plugin declares
  `continues = 10`.
- Rules are checked when added or edited, and again when read back from
  the store. Stored rules that do not check fail the run at start, saying
  why (`tau.fail`): broken rules are never silently ignored. That needs a
  TypeSafe key; without one nothing is checked. None stored is no rules.
- The plugin reads the rules from the store at every check, so an edit
  applies from the next tool call, in runs already going too.

## What happens

| Where        | Violation probability       | Result                                                                                                                              |
| ------------ | --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Tool call    | ≥ `block`                   | Refused (`tau.block`). The model gets the rule, quoted, and fixes the call.                                                         |
| Tool call    | ≥ `review`                  | Runs, flagged for review.                                                                                                           |
| Final answer | ≥ `block`                   | Sent back with the rule (`tau.continue`), up to `max_holds` times.                                                                  |
| Final answer | ≥ `review`, or past the cap | Stands, flagged.                                                                                                                    |
| Jev fails    | —                           | `on_error`: `allow` lets the call run or the answer stand; `block` refuses the call, or sends the answer back while holds are left. |

Jev sees only the fields a rule names, as the model wrote them: never
tool output or file content, which could try to steer it. All rules on
one call go in one request, one `noul` per rule (`rule-0`, `rule-1`…);
its cost is charged to the run. A Luau table keeps no order, so where a
field is found under several keys of one object, its values come in
the keys' sorted order.

## Records and state

Each check makes records, as the Rust plugin did: a `checked` record
(the call and tool, none for the final answer, each rule asked with its
score, and what Jev cost), then a verdict per rule past its review mark
(`blocked`, `flagged` or `held`: the rule, its text, the score, its
marks, the call and tool, the reason given, and for a hold which one it
is out of `max_holds`), or an `error` when Jev failed (the message,
`on_error`, and for a final answer whether it was sent back). They are
folded into the run's state (`checks.fold`), which is stored with the
run, so history shows it again:

- `stats`: calls and answers checked, questions asked, failures, Jev's
  cost, and the rules behind each block, flag and hold;
- `calls`: each checked call's scores and verdict, by call id;
- `notes`: a note for each failure and each verdict on the answer;
- `said`: the messages it sent back, which draw as nothing (its note
  says it);
- `starting`: what it says before anything is checked: off without a
  key, else how many rules it watches.

## In tau-ui

- The repository's page (`pages.rules`, its entry under each repository
  in the sidebar with how many rules and what waits for review):
  - **Rules:** each rule's places, strictness, what it last caught and
    what it did over the repository's runs, from the runs' counts in the
    store.
  - **Editor:** writes and edits a rule. Enter in the rule's field keeps
    the rule; places are picked from a list, or any `tool.field` typed
    and entered. Lenient / Balanced / Strict thresholds (flag/block at
    0.5/0.9, 0.3/0.8, 0.2/0.6) or steps of 0.05, and **Try it**: Jev
    is asked about the repository's latest calls and answers the rule
    reads, with the same question and state a check uses, one request
    each, and nothing runs again. The page's actions have the host's Jev
    (`ctx.jev`).
  - **Review:** flagged calls and answers, with the rule and the score
    against its thresholds; Looks fine takes one off the queue, and Open
    in run opens its run (`tau.open_run`). Next to it, what the rules
    handled on their own.
  - **Settings:** what happens when Jev cannot answer (`on_error`), and
    how many times one run's answer may be sent back (`max_holds`, 0 to
    10).
  - **Rules that cannot be read:** why, and that runs in the repository
    fail at start until they can (once there is a TypeSafe key). No edit
    can fix them, so the banner offers to remove them, once confirmed, to
    start again.
  - Without a TypeSafe key the page says so and links to tau-jev's key
    page; it knows from `ctx.run.jev`.
- Checks need a TypeSafe key, added in tau-jev's section of the Models
  screen and kept by tau-jev in tau's config directory (`typesafe-key`),
  readable only by the user. Without one, runs are not checked and the
  status line says so.
- On a call's card (the view's `calls`): a flagged call shows its rule,
  score and a Review link beside its title, and is marked flagged; a
  blocked one shows the rule and Jev's score against its marks under it,
  and is marked blocked with the reason the model got; a call that
  passed shows each rule's score. Live and in history.
- A held answer shows as a note with its count (`continuation 1 / 3`).
- The inspector says what the checks did in the run (calls and answers
  checked, questions asked, blocks, flags and holds with their rules,
  Jev's cost), and a finished run's links to its first flagged call's
  rule.
