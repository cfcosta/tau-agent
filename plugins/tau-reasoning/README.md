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

It is a Luau plugin tau comes with (ADR 0050, 0051): `src/plugin.luau` and
`src/effort.luau`, loaded as always active. It reaches Jev on runs on auto,
when a TypeSafe key is saved.

## Usage

The `replay` example runs the policy over the latest runs of a store,
each on its own model, with the real Jev. It needs `TYPESAFE_API_KEY`
and the network:

```sh
cargo run -p tau-built-in-tests --example replay -- ~/.local/share/tau/runs.db 20
```

It reports decisions, not savings: what a request would have cost at
another effort is unknown.

## Testing

```sh
target/release/tau --plugin-test plugins/tau-reasoning   # its Luau tests, no compiling
cargo nextest run --release -p tau-built-in-tests -E 'test(/^reasoning::/)'
```

The tests use a fake Jev, so they need no key and no network.

## Further reading

- [plugins.md](../../docs/reference/plugins.md): the
  `tau-reasoning` section, including "Replaying stored runs", and the
  shared Jev client
- [openai-websocket.md](../../docs/reference/openai-websocket.md):
  why a change of effort costs the prompt cache
- [0022-the-prompt-cache-follows-the-connection.md](../../docs/decisions/0022-the-prompt-cache-follows-the-connection.md):
  sticky effort
- [0017-plugins-bring-their-ui.md](../../docs/decisions/0017-plugins-bring-their-ui.md)
  and [0030-host-halves-are-crates.md](../../docs/decisions/0030-host-halves-are-crates.md):
  its UI and host half
