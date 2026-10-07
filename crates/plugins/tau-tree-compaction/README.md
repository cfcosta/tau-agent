# tau-tree-compaction

Compaction into a tree of one-line summaries the agent can zoom into,
after OptChat
(<https://gist.github.com/VictorTaelin/91837951a5ce5b38f341ec1ba1df6449>).
The rules are in
[docs/reference/tree-compaction.md](../../../docs/reference/tree-compaction.md).

- `tree`: the history without I/O. It holds:
  - `Entry` and `entries`, a message as the history keeps it;
  - `Node`, a line of the tree, named `id+n`;
  - `History`, with `fit` (merge the most due pair until the view fits),
    `wanted` (the merges `fit` will need), `render` and `zoom`.
- `build`: the compactor. `Builder::grow` builds every line a view
  needs, `jobs` requests at once. `COMPACT_PROMPT` is OptChat's prompt,
  for a run. `SCALE` is a line of exactly 512 bytes.
- `TreeCompaction`: the plugin, and the `zoom` tool it adds to each run.
- `Record`, `Details`: what it stores, so a fork can zoom.
- `ui::TreeCompactionUi` and `ui::TreeCompactionHost`: its switch, off
  by default, and its host half.

```rust
let agent = Agent::new(llm)
    .plugin(TreeCompaction::default())
    .plugin(tau_compaction::Compaction::default()); // when a line fails
```
