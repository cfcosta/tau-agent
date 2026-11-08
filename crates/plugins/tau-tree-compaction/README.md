# tau-tree-compaction

Compaction into a tree of one-line summaries the agent can zoom into,
after OptChat. Near the window, the messages before the kept tail are
folded into a history kept word for word, a binary tree of summaries
grows over it, and its view, lines that tile the whole history within a
byte budget, takes their place. The `zoom` tool opens any line into the
two it was made from, down to a message whole. It is off by default.

It is a Luau plugin tau comes with (ADR 0050): `luau/lib/tree.luau`
holds the history, the tree and the view, `luau/lib/build.luau` builds
the lines with the model, and `luau/plugin.luau` declares the hooks and
`zoom`. tau-luau-plugins loads it as always active, after fast pruning
and before tau-compaction, which summarizes when a line cannot be
built. Its rules are in
[tree-compaction.md](../../../docs/reference/tree-compaction.md).

## What it provides

| Item               | What it is                                      |
| ------------------ | ----------------------------------------------- |
| `NAME`             | `"tau-tree-compaction"`                         |
| `FILES`            | The Luau files, by their paths in its folder    |
| `DEMO_DECLARATION` | What it declares, as the demo lists it, as JSON |

## Testing

```sh
cargo nextest run --release -p tau-tree-compaction
```

`luau/tests/tree.luau` holds properties of names, fitting, what to
build, zoom, render and restore; `luau/tests/build.luau` grows lines
against a stand-in model; `luau/tests/plugin.luau` runs the hooks
against fake runs. `tests/luau.rs` loads the plugin as tau does and runs
them, and checks the demo's fixture of its declaration
(`BLESS_FIXTURES=1` writes it anew). `tests/in_the_loop.rs` runs it in
the agent loop with `ScriptedModel`: folding, a second compaction,
forks that zoom, waits after a failure, tau-compaction stepping in, and
how many lines are asked at once.
