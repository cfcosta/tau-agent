# tau-ask

Lets an agent ask the person structured questions and wait for the
answers. The `ask` tool takes one to four questions, each with two to
four choices, one or several to pick, and a preview per choice. The
person can always write their own answer instead, and add a note to any
answer. The call waits until the answers come, the person declines, or
the run is cancelled; the answers are the call's result, so the turn
goes on without a new message. It is a Luau plugin tau comes with (ADR
0050): this crate carries its files, what the demo shows of it, and the
tests that check them.

## What it provides

| Item               | What it is                                                                |
| ------------------ | ------------------------------------------------------------------------- |
| `NAME`             | `"tau-ask"`, the plugin's name and its folder's                           |
| `FILES`            | Its Luau files by their paths in its folder, which tau-luau-plugins loads |
| `DEMO_ASK`         | The demo's run waiting on three questions: its state and view, untouched  |
| `DEMO_NOTE`        | The same, a checklist's note being written                                |
| `DEMO_REVIEW`      | The same, on the review                                                   |
| `DEMO_SWEEP`       | Another chat waiting on one question, as the sidebar shows it             |
| `DEMO_DECLARATION` | What the plugin declares, as the demo lists it among tau's Luau plugins   |

The Luau, under `luau/`:

| File               | What it is                                                                     |
| ------------------ | ------------------------------------------------------------------------------ |
| `plugin.luau`      | The plugin: the `ask` tool, the panel's actions, `start`, `run_end`, its view  |
| `lib/ask.luau`     | The limits and their check, whether answers fit, what the model reads, a draft |
| `lib/escaped.luau` | The characters a quoted header writes as `\u{...}`, as Rust's `{:?}` does      |
| `lib/panel.luau`   | The panel in the composer's place, and the card                                |
| `tests/*.luau`     | Its tests: cases, properties and a state machine, against fake runs            |

## How it fits

tau-luau-plugins-host lists it among the Luau plugins tau comes with
(`BUILT_IN`), always active. Its tool answers `tau.ask`, and the host
holds the call until one of its actions answers `tau.answer` (ADR 0050,
gap C); its view's `composer` is the panel. An agent's runs leave it
out: they ask through tau-agents' own `ask` (ADR 0037). `tau-demo`
draws it from the fixtures, as the demo runs no Luau; this crate's
tests check each fixture is what the plugin leaves and draws.

## Testing

```sh
cargo nextest run --release -p tau-ask
```

`tests/luau.rs` loads the plugin as tau does and runs its Luau tests,
and checks the fixtures (`BLESS_FIXTURES=1` writes them anew).
tau-ui's `tests/ask.rs` runs it in a host, answered from two devices,
and draws its panel in the workspace.

## Further reading

- [docs/reference/ask.md](../../../docs/reference/ask.md)
- [0019: The agent asks the person, in the composer's place](../../../docs/decisions/0019-ask-the-person.md)
- [0050: tau's own plugins, in Luau](../../../docs/decisions/0050-tau-s-plugins-in-luau.md)
