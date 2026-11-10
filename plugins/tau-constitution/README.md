# tau-constitution

Checks a tau run's tool calls and final answer against the repository's
rules, asking Jev how likely each rule is broken. A call past a rule's
block mark is refused with the rule, so the model can fix it; one
between its marks runs, flagged for a person; a final answer that
breaks a rule goes back, up to `max_holds` times. It is a Luau plugin
tau comes with (ADR 0050): this folder is its package, with the tests and the fixtures the demo
shows of it.

## What it provides

| Item               | What it is                                                                |
| ------------------ | ------------------------------------------------------------------------- |
| `NAME`             | `"tau-constitution"`, the plugin's name and its folder's                  |
| `FILES`            | Its Luau files by their paths in its folder, which tau-luau-plugins loads |
| `DEMO_RUN`         | The state and view after each check of the demo's run                     |
| `DEMO_PAGES`       | The demo's rules page in each state its screens show                      |
| `DEMO_DECLARATION` | What the plugin declares, as the demo lists it among tau's Luau plugins   |

The Luau, under `src/`:

| File              | What it is                                                                        |
| ----------------- | --------------------------------------------------------------------------------- |
| `plugin.luau`     | The plugin: `start`, `before_tool`, `before_stop`, its view, its page and actions |
| `lib/rules.luau`  | Rules: targets, checking a rule, reading one back, editing, what a rule reads     |
| `lib/checks.luau` | What Jev is asked, the records a check makes, their fold, the counts              |
| `lib/page.luau`   | The repository's page: the editor's draft, Try it, and the page's tree            |
| `tests/*.luau`    | Its tests: cases, properties and a state machine, against fake runs               |

A rule applies to tool arguments (`edit.newText` is every `newText` in
an `edit` call's arguments, however deep) or to the final answer
(`final answer`). The rules, the review queue and what the checks did
live in the plugin's store in each repository, and the page edits them.

## How it fits

tau-luau-plugins-host lists it among the Luau plugins tau comes with
(`BUILT_IN`), always active. It needs a TypeSafe key: without one,
nothing is checked. `tau-demo` draws it from the fixtures, as the demo
runs no Luau; `tau-plugin test` checks each fixture is what the plugin
leaves and draws.

## Testing

```sh
target/release/tau-plugin test plugins/tau-constitution   # its Luau tests, no compiling
target/release/tau-plugin test plugins/tau-constitution --bless   # writes its fixtures anew
cargo nextest run --release -p tau-built-in-tests -E 'test(/^constitution::/)' --run-ignored only
```

`tau-plugin test` loads the plugin as tau does and runs its Luau tests.
`tests/fixtures.luau` makes the demo's fixtures, which it holds to
`fixtures/`; a case there checks the demo run's stats and marks, and the
store's review item and answers.
`tau-built-in-tests/tests/constitution/live.rs` asks the real Jev and is
ignored without `TYPESAFE_API_KEY` (`--run-ignored only`). tau-ui's
`tests/constitution.rs` runs it in a host.

## Further reading

- [docs/reference/constitution.md](../../docs/reference/constitution.md)
- [docs/reference/plugins.md](../../docs/reference/plugins.md), section
  "`tau-constitution`: rules checked on specific calls"
- [ADR 0050: tau's own plugins, in Luau](../../docs/decisions/0050-tau-s-plugins-in-luau.md)
