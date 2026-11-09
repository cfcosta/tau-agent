# tau-compaction

Summarizing compaction for tau runs, a port of pi's coding-agent
compaction. When a run's context nears the model's window, the older
messages are summarized in a request of their own and replaced by the
summary, the newest kept as they are. A context overflow summarizes at
once, and an idle chat is summarized in its own conversation, before its
prompt cache lapses.

It is a Luau plugin tau comes with (ADR 0050): `src/plugin.luau`
declares its hooks, `src/compaction.luau` holds the rules, and
tau-luau-plugins loads it as always active, after fast pruning and the
tree. Its rules are in
[compaction.md](../../docs/reference/compaction.md).

## What it provides

| Item               | What it is                                      |
| ------------------ | ----------------------------------------------- |
| `NAME`             | `"tau-compaction"`                              |
| `FILES`            | The Luau files, by their paths in its folder    |
| `DEMO_DECLARATION` | What it declares, as the demo lists it, as JSON |

An agent of its own, as an evaluation's, runs it with
`tau_luau_plugins_host::agent::Active::built_in(NAME)` in a
`LuauPlugins`.

## Testing

```sh
target/release/tau --plugin-test plugins/tau-compaction   # its Luau tests, no compiling
cargo nextest run --release -p tau-built-in-tests -E 'test(/^compaction::/)'
```

`tests/rules.luau` holds properties of the threshold, the cut point
and the plan, the serialized conversation, the file lists and the
prompts, and pi's known cases. `tests/plugin.luau` runs the hooks
against fake runs. `tau-built-in-tests/tests/compaction/luau.rs` loads the plugin as tau does and runs
them, and checks the demo's fixture of its declaration
(`BLESS_FIXTURES=1` writes it anew). `tau-built-in-tests/tests/compaction/in_the_loop.rs` runs it in
the agent loop with `ScriptedModel`: threshold and overflow compaction,
split turns, rejected summaries and their wait, forks, retries and idle
chats.
