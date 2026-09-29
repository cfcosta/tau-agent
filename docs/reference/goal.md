# tau-goal

Keeps a conversation going until a goal holds. Each time the model
would stop, Jev (TypeSafe's System One model) is asked whether the
goal is met; if not, the model is sent back with the goal. Crate:
`crates/plugins/goal`. Design: [plugins.md](plugins.md).

## Setting a goal

A run's input sets it:

```text
/goal [--continuations N] [--budget USD] <condition>
/goal clear
```

- The condition is plain words, checkable from what the model runs:
  "cargo nextest run -p tau-ai passes and clippy is clean". Its words
  are joined by single spaces, so line breaks and runs of spaces go.
- A condition written as one quoted string, `"..."`, is read exactly:
  `\"` is a quote, `\\` a backslash, `\n` a line feed and `\r` a
  carriage return. So `/goal "clear"` sets the goal `clear`, and
  `/goal "--budget 5 is spent"` a goal that starts with `--budget`.
  Quotes that do not wrap the whole condition are plain characters.
- `--continuations` (default 10) is how many times the goal may send
  the model back. `--budget` (default $2.00) caps what the goal costs:
  the run's turns and the checks, from when it was set.
- The model does not see the command. Its input is `/goal <condition>`
  and a paragraph on how goals work (`tau_goal::INSTRUCTIONS`). The
  condition is written as is when it reads back as itself, and quoted
  when it does not (it is `clear`, starts with a limit, spans lines,
  has runs of whitespace, or is itself one quoted string), so an
  interface reads every condition back exactly.
- A goal belongs to the conversation. Resuming the run keeps it, and a
  new `/goal` replaces it. `/goal clear` removes it.

## The check

When the model answers without tool calls, and the goal is active:

1. Jev gets one `Noul` question, "is the goal met?", on a state of the
   goal, the model's last answer, and its last 6 tool results (the last
   1,500 characters of each output). The answer's own claims count only
   where a result backs them.
2. At 0.7 or more the goal is **met**, and the run stops.
3. Below it, the model is sent back, as a user message that starts with
   `tau-goal: ` and names the goal and the continuation. Out of
   continuations, or past the budget, the goal **stops** instead, and
   so does the run.
4. When Jev gives no answer, the run stops unchecked, and the error is
   recorded. The goal stays active for the next stop.

The agent's `Limits::max_continuations` still applies to all plugins
together; tau's interface lifts it, since each plugin caps itself.

## Records

Everything that happens to a goal is a record, stored with the run and
reported as it happens (`tau_goal::Record`):

| `kind`     | Fields                                                   | Effect                           |
| ---------- | -------------------------------------------------------- | -------------------------------- |
| `set`      | `goal`, `continuations`, `budget`                        | a new active goal                |
| `check`    | `n`, `met`, `p`, `turn`, `continuation`, `cost`, `spent` | one check; `met` ends it         |
| `stopped`  | `why`: `continuations` or `budget`                       | stops it, not met                |
| `extended` | `by`                                                     | more continuations, active again |
| `paused`   |                                                          | kept, not checked                |
| `resumed`  |                                                          | checked again                    |
| `cleared`  |                                                          | no goal                          |
| `error`    | `message`                                                | the last check could not run     |

`tau_goal::Goal::fold` turns a run's records into its goal. The plugin
folds them again at every check (`PluginCtx::records`), so an
interface controls a goal by storing a record with the run: tau's
Pause, Resume, Keep going and Clear do.
