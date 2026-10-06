# tau-reasoning

Picks a tau run's reasoning effort from its task with Jev, as a plugin.
Only runs that left the effort open ("auto", `plan.reasoning` is `None`)
are scored; an effort someone chose stands. Jev gets the task, clipped
at both ends, and the start of the instructions, and answers one
question whose levels say what each effort suits. A confident answer
sets the effort. Otherwise the message runs at the effort the
conversation last ran at, or the model's default.

A long conversation keeps the effort it last ran at, even when Jev is
sure of another: no model keeps its prompt cache across a change of
effort, so a switch would resend the whole prefix uncached. For the same
reason the effort stays fixed while a run works, unless `redecide` is
on.

## What it provides

- `Reasoning`: the plugin. `Reasoning::new(jev)` takes an
  `Arc<dyn Jev>`; `threshold` (default `DEFAULT_THRESHOLD`, 0.7),
  `sticky_after` (default `STICKY_TOKENS`, 20,000), `redecide` (off by
  default) and `levels` change how it picks. `picker(model)` gives the
  `Picker` for one model.
- `Picker`: asks Jev for one model's effort. `scores()` is false when
  the model has no efforts to choose from.
- `Level` and `levels_for`: the efforts a model takes, lowest first,
  each with the work it suits.
- `Lease`: with `redecide`, how long a chosen effort holds
  (`OneCall`, `ToolChain`, `UserTurn`). A failed tool call, a context
  rewrite or a new user message ends any lease.
- `Choice`, `Verdict`, `Scored`, `Context` and `Record`: what the plugin
  reports and records, so interfaces can show why a message ran at its
  effort. `Choice::kept_for_cache` says the effort stayed for the cache.
- `runs_at` and `last_effort`: the rule for the effort a message runs
  at, and the effort the conversation last ran at, from the plugin's
  records along the fork chain.
- `replay`: walks a stored run's timeline through the same policy and
  reports what it would decide before each request (`replay::replay`,
  `replay::Entry`, `replay::Decision`).
- `NAME`: `"tau-reasoning"`.
- `ReasoningPlugin` and `ReasoningHost` (from `ui`): its UI and its host
  half. `ui::Settings` holds `redecide` and `threshold`, as set on tau's
  Models screen.

## How it fits

It is a single crate: the plugin, its `UiPlugin` and its `HostHalf` live
here. It builds on `tau-agent` (the `start`, `before_request`,
`rewritten` and `finish` seams), `tau-ai` (the efforts a model takes)
and `tau-jev`, the shared Jev client. Its UI uses `tau-ui-kit`,
`tau-ui-plugin` and gpui.

`tau-ui` registers `ReasoningHost`, which adds the plugin to a run only
when the host has a Jev (a saved TypeSafe key) and the run's effort is
on auto. `tau-ui-remote` registers `ReasoningPlugin`.

## Usage

```rust
use std::sync::Arc;

use tau_agent::agent::Agent;
use tau_jev::TypeSafe;
use tau_reasoning::Reasoning;

let jev = Arc::new(TypeSafe::from_env()?); // reads TYPESAFE_API_KEY
let agent = Agent::new(llm)
    .instructions("You are a careful coding agent.")
    .plugin(Reasoning::new(jev).threshold(0.8));
```

Share one `Reasoning` across an agent's runs. Each scoring is one Jev
round trip before the run's session opens, and with `redecide` one more
per ended lease.

The `replay` example runs the policy over the latest runs of a store,
each on its own model, with the real Jev. It needs `TYPESAFE_API_KEY`
and the network:

```sh
cargo run -p tau-reasoning --example replay -- ~/.local/share/tau/runs.db 20
```

It reports decisions, not savings: what a request would have cost at
another effort is unknown.

## Testing

```sh
cargo nextest run --release -p tau-reasoning
```

The tests use `tau_jev`'s `FakeJev`, so they need no key and no
network. `tests/reasoning.rs` covers the policy, with Hegel properties,
through the agent loop with `ScriptedModel`. `tests/ui.rs` covers what
the UI's fold makes of the plugin's records.

## Further reading

- [plugins.md](../../../docs/reference/plugins.md): the
  `tau-reasoning` section, including "Replaying stored runs", and the
  shared Jev client
- [openai-websocket.md](../../../docs/reference/openai-websocket.md):
  why a change of effort costs the prompt cache
- [0022-the-prompt-cache-follows-the-connection.md](../../../docs/decisions/0022-the-prompt-cache-follows-the-connection.md):
  sticky effort
- [0017-plugins-bring-their-ui.md](../../../docs/decisions/0017-plugins-bring-their-ui.md)
  and [0030-host-halves-are-crates.md](../../../docs/decisions/0030-host-halves-are-crates.md):
  its UI and host half
