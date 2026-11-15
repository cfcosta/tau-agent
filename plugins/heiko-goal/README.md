# heiko-goal

Keeps a heiko run going until a goal holds. A run whose input is
`/goal <condition>` sets the goal. Each time the model would stop, the decider
is asked whether the goal holds, judged from the model's last answer and
its recent tool results. If not, the model is sent back with the goal,
within the goal's continuations and budget. The goal lives in the run's
records, so it outlives the process and an interface can pause, extend
or clear it.

## What it provides

| Item                    | What it is                                                                    |
| ----------------------- | ----------------------------------------------------------------------------- |
| `GoalPlugin`            | The `Plugin`: `GoalPlugin::new(decide)`; share one across an agent's runs        |
| `Command`               | A parsed `/goal`: `Set { condition, continuations, budget }` or `Clear`       |
| `Command::parse`        | Reads `/goal [--continuations N] [--budget TOKENS] <condition>` or `/goal clear` |
| `set_input`             | The input the model gets for a goal: the condition, then `INSTRUCTIONS`       |
| `set_message`           | The condition of a message that set a goal, read back exactly                 |
| `Record`                | Everything that happens to a goal, as stored and reported                     |
| `Goal`                  | A conversation's goal, folded from its records (`Goal::fold`, `apply`)        |
| `Status`, `Exhausted`   | Active, paused, met or stopped; and why it stopped                            |
| `Check`                 | One check: the decider's probability, the turn, the continuation, the tokens spent    |
| `DEFAULT_CONTINUATIONS` | 10                                                                            |
| `DEFAULT_BUDGET`        | 2m tokens, for the run's turns from when the goal was set                     |
| `MET_AT`                | 0.7: the probability at or past which the goal counts as met                  |
| `CONTINUATION_PREFIX`   | `"heiko-goal: "`, how a continuation message starts                             |
| `GoalUi`, `GoalHost`    | The `UiPlugin` and the `HostHalf` heiko's interface registers                   |
| `ui::Act`, `ui::act`    | The goal's buttons: pause, edit, clear, keep going                            |
| `demo::records`         | The goals heiko-ui's demo shows (`demo` feature)                                |

`Record` has one variant per `kind`: `set`, `check`, `stopped`,
`extended`, `paused`, `resumed`, `cleared`, `error`, and `starting`,
which the interface folds as a run starts and is never stored. The
plugin folds a run's records again at every check, so an interface
controls a goal by storing a record with the run.

## How it fits

It is a Luau plugin heiko comes with (ADR 0050, 0051): `src/plugin.luau` and
`src/goal.luau`, loaded as always active. It reaches the decider, so it does nothing
without a saved TypeSafe key, and it is not in a sub-agent's runs.

## Usage

`/goal --budget 500k cargo nextest run passes and clippy is clean`

A later run that resumes the conversation keeps the goal. `/goal clear`
removes it, and a new `/goal` replaces it.

## Testing

```sh
target/release/heiko-plugin test plugins/heiko-goal   # its Luau tests, no compiling
target/release/heiko-plugin test plugins/heiko-goal --bless   # writes its fixtures anew
```

`tests/plugin.luau` drives fake runs with a fake decider, so no TypeSafe
key or network is needed. `tests/command.luau` holds properties: a
written `/goal` reads as itself, budgets are whole numbers of tokens,
and any condition reads back from the model's input.
`tests/fixtures.luau` makes the demo's fixtures, which `heiko-plugin test`
holds to `fixtures/`.

## Further reading

- [docs/reference/goal.md](../../docs/reference/goal.md)
- [docs/reference/plugins.md](../../docs/reference/plugins.md), section
  "`heiko-goal`: keep going until a goal holds"
- [ADR 0017: plugins bring their UI](../../docs/decisions/0017-plugins-bring-their-ui.md)
- [ADR 0030: host halves are crates](../../docs/decisions/0030-host-halves-are-crates.md)
