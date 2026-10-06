# tau-goal

Keeps a tau run going until a goal holds. A run whose input is
`/goal <condition>` sets the goal. Each time the model would stop, Jev
is asked whether the goal holds, judged from the model's last answer and
its recent tool results. If not, the model is sent back with the goal,
within the goal's continuations and budget. The goal lives in the run's
records, so it outlives the process and an interface can pause, extend
or clear it.

## What it provides

| Item                    | What it is                                                                    |
| ----------------------- | ----------------------------------------------------------------------------- |
| `GoalPlugin`            | The `Plugin`: `GoalPlugin::new(jev)`; share one across an agent's runs        |
| `Command`               | A parsed `/goal`: `Set { condition, continuations, budget }` or `Clear`       |
| `Command::parse`        | Reads `/goal [--continuations N] [--budget USD] <condition>` or `/goal clear` |
| `set_input`             | The input the model gets for a goal: the condition, then `INSTRUCTIONS`       |
| `set_message`           | The condition of a message that set a goal, read back exactly                 |
| `Record`                | Everything that happens to a goal, as stored and reported                     |
| `Goal`                  | A conversation's goal, folded from its records (`Goal::fold`, `apply`)        |
| `Status`, `Exhausted`   | Active, paused, met or stopped; and why it stopped                            |
| `Check`                 | One check: Jev's probability, the turn, the continuation, what it cost        |
| `DEFAULT_CONTINUATIONS` | 10                                                                            |
| `DEFAULT_BUDGET`        | $2.00, for the run's turns and the checks from when the goal was set          |
| `MET_AT`                | 0.7: the probability at or past which the goal counts as met                  |
| `CONTINUATION_PREFIX`   | `"tau-goal: "`, how a continuation message starts                             |
| `GoalUi`, `GoalHost`    | The `UiPlugin` and the `HostHalf` tau's interface registers                   |
| `ui::Act`, `ui::act`    | The goal's buttons: pause, edit, clear, keep going                            |
| `demo::records`         | The goals tau-ui's demo shows (`demo` feature)                                |

`Record` has one variant per `kind`: `set`, `check`, `stopped`,
`extended`, `paused`, `resumed`, `cleared`, `error`, and `starting`,
which the interface folds as a run starts and is never stored. The
plugin folds a run's records again at every check, so an interface
controls a goal by storing a record with the run.

## How it fits

tau-goal is a single crate: the interface half and the host half live
together, since neither pulls in anything heavy (decision 0030 keeps
only heavy host halves in crates of their own). It builds on `tau-agent`,
`tau-ai`, `tau-jev`, `tau-ui-plugin` and `tau-ui-kit`, and on gpui.

`tau-ui` registers `GoalHost`, which adds the plugin to a run only when
its services hold a Jev (a TypeSafe key is saved), and never to a
sub-agent. `tau-ui-remote` lists `GoalUi`, so a phone shows goals too.

## Usage

```rust
use std::sync::Arc;
use tau_agent::limits::Limits;
use tau_goal::GoalPlugin;
use tau_jev::{Jev, TypeSafe};

let jev: Arc<dyn Jev> = Arc::new(TypeSafe::from_env()?);
let agent = agent
    .plugin(GoalPlugin::new(jev))
    // The agent's own cap on continuations applies to all plugins
    // together; the goal caps itself.
    .limits(Limits::default().max_continuations(100));

let task = "/goal --budget 5 cargo nextest run passes and clippy is clean";
let outcome = agent.run(task, &store).await?;
```

A later run that resumes the conversation keeps the goal. `/goal clear`
removes it, and a new `/goal` replaces it.

## Features

| Feature | What it adds                                   |
| ------- | ---------------------------------------------- |
| `demo`  | `demo::records`: the goals tau-ui's demo shows |

## Testing

```sh
cargo nextest run --release -p tau-goal
```

The tests drive real runs with `tau-testing`'s `ScriptedModel`,
`tau_jev::fake::FakeJev` and a store, so no TypeSafe key or network is
needed. Property tests (`hegeltest`) check that a written `/goal` reads as
itself, that budgets are finite and not below zero, that any condition
reads back from the model's input, and that the UI's fold follows the
records.

## Further reading

- [docs/reference/goal.md](../../../docs/reference/goal.md)
- [docs/reference/plugins.md](../../../docs/reference/plugins.md), section
  "`tau-goal`: keep going until a goal holds"
- [ADR 0017: plugins bring their UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [ADR 0030: host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
