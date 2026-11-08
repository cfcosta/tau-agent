# tau-fast-compaction

Keeps a tau run's context lean with Jev, TypeSafe's System One model, as
a plugin. It never summarizes: what stays is verbatim, and what goes is
archived to a file or can be re-run. When pruning cannot free enough,
the tree or the summary (`tau-tree-compaction`, `tau-compaction`, after
it) takes over.

It works in two stages:

- Output pruning, at `after_tool_result`. A large `exec_command` or `write_stdin` result is
  trimmed to the chunks of lines Jev says the task still needs, before
  the model first sees it. The whole output is archived to a file the
  model can read. Nothing already sent changes, so it costs no resend.
- History pruning, between turns. Once the context passes a share of the
  window, and on overflow, Jev decides for each tool call that is not
  pinned whether the call and its full result still matter. Each call is
  kept, has its result cut to a head and a note naming its archive, or
  goes with its result. When the pass saves enough, the pruned
  transcript replaces the run's as a context rewrite.

History pruning ports `joelhooks/pi-fast-jev-compaction`, itself built
on `tamaratran/fast-jev-compaction`. Output pruning, the partitioned
history and the token estimate come from `tamaratran/jev-pruner`. All
three are MIT; the notices are in `THIRD_PARTY_NOTICES.md`.

It is a Luau plugin tau comes with (ADR 0050): `luau/lib/` holds the
estimate, the history state, its partitions, the decisions, the ledger
and output pruning, and `luau/plugin.luau` declares the hooks and draws
the status line, the notes, the cards and the ledger page.
tau-luau-plugins loads it as always active, first of the compactions.
Its rules are in
[fast-compaction.md](../../../docs/reference/fast-compaction.md).

## What it provides

| Item               | What it is                                             |
| ------------------ | ------------------------------------------------------ |
| `NAME`             | `"tau-fast-compaction"`                                |
| `FILES`            | The Luau files, by their paths in its folder           |
| `HEADER`           | The first line of a pruned output                      |
| `DEMO_DECLARATION` | What it declares, as the demo lists it, as JSON        |
| `DEMO_RUN`         | The demo run's three steps, as it keeps and draws them |

`tau-output-pruning-eval` measures output pruning against the real Jev.

## Testing

```sh
cargo nextest run --release -p tau-fast-compaction
```

`luau/tests/estimate.luau` checks the token estimate;
`luau/tests/rules.luau` holds properties of the history state, its
segments, the decisions and the ledger; `luau/tests/output.luau` those
of output pruning's gate, chunks, plan, answers and render;
`luau/tests/plugin.luau` runs the hooks and the view against fake runs.
`tests/luau.rs` loads the plugin as tau does and runs them, and checks
the demo's fixtures (`BLESS_FIXTURES=1` writes them anew).
`tests/in_the_loop.rs` and `tests/output_in_the_loop.rs` run it in the
agent loop with `ScriptedModel` and a fake Jev: passes, cooldowns,
forks, overflows, failures and archives, and outputs pruned, kept and
left alone.
