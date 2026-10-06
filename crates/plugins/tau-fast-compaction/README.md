# tau-fast-compaction

Keeps a tau run's context lean with Jev, TypeSafe's System One model, as
a plugin. It never summarizes: what stays is verbatim, and what goes is
archived to a file or can be re-run. When pruning cannot free enough,
summarizing compaction (`tau-compaction`, added after it) takes over.

It works in two stages:

- Output pruning, at `after_tool_result`. A large `bash` result is
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

## What it provides

- `FastCompaction`: the plugin. `FastCompaction::new(jev)` takes any
  `Jev`, `FastCompaction::shared` takes an `Arc<dyn Jev>` shared with
  other plugins, and `.settings(..)` replaces the defaults.
- `Settings`: history pruning's settings (pi's defaults), such as
  `compact_at_percent` (60), `min_reduction_ratio` (0.25),
  `preserve_recent` (6), `context_window` and `archive_dir` (the system
  temporary directory by default).
- `OutputPruning`: output pruning's settings, in `Settings::output`.
  On by default; only outputs over `min_output_tokens` (10,000) are
  touched.
- `Record`, `Details` and `Stats`: what the plugin publishes and stores,
  per pruned output and per pass, Jev's cost included.
- `Ledger`, `Decision` and `Action`: the decisions so far, which only
  ever escalate. A fork inherits them from the stored rewrite.
- `PruneError`: why a pass or an output pruning failed. A failure is
  reported and leaves the transcript as it was.
- `NAME`: `"tau-fast-compaction"`.
- Modules with the pieces: `state` (the transcript as Jev sees it, and
  the token estimate), `history` (splitting a long history into
  segments), `plan` (which requests to send), `decide` (asking Jev about
  tool calls), `ledger`, `output` (output pruning) and `archive` (the
  archive files).
- `ui::FastCompactionUi` and `ui::FastCompactionHost`: its UI (what each
  pass left of a call, its ledger page, its place on the context meter)
  and its host half.

## How it fits

It is a single crate: the plugin, its `UiPlugin` and its `HostHalf` live
here. It builds on `tau-agent` (the `rewrite_context` and
`after_tool_result` seams), `tau-ai` and `tau-jev`, the shared Jev
client. Its UI uses `tau-ui-kit`, `tau-ui-plugin` and gpui.

`tau-ui` registers `FastCompactionHost`, which adds the plugin to a run
only when the host has a Jev (a saved TypeSafe key), and archives to
`archive/` in tau's directory for the repository. `tau-ui-remote`
registers `FastCompactionUi`. `tau-output-pruning-eval` measures output
pruning through the real plugin.

## Usage

Add it before summarizing compaction:

```rust
use tau_agent::agent::Agent;
use tau_compaction::Compaction;
use tau_fast_compaction::FastCompaction;
use tau_jev::TypeSafe;

let agent = Agent::new(llm)
    .plugin(FastCompaction::new(TypeSafe::from_env()?))
    .plugin(Compaction::default()); // after: it takes what pruning declines
```

`TypeSafe::from_env` reads `TYPESAFE_API_KEY`. To change a setting, keep
the rest at their defaults:

```rust
use tau_fast_compaction::{FastCompaction, Settings};

let pruning = FastCompaction::new(TypeSafe::from_env()?).settings(Settings {
    archive_dir: "/var/tmp/tau-archive".into(),
    ..Settings::default()
});
```

## Testing

```sh
cargo nextest run --release -p tau-fast-compaction
```

The tests use `tau_jev`'s `FakeJev` with answers the tests choose, so
they need no key and no network. `tests/rules.rs` and `tests/output.rs`
hold Hegel property tests of the pure parts. `tests/in_the_loop.rs` and
`tests/output_in_the_loop.rs` run the plugin through the agent loop with
`ScriptedModel` and `tau_store_sqlite::memory()`, including beside
`tau-compaction`. `tests/ui.rs` tests the UI's fold.

## Further reading

- [fast-compaction.md](../../../docs/reference/fast-compaction.md): both
  stages, their costs, failures, settings, the evaluation and its
  results, and where it differs from pi and jev-pruner
- [compaction.md](../../../docs/reference/compaction.md): the
  summarizing compaction that runs after it
- [plugins.md](../../../docs/reference/plugins.md): the plugin seams,
  context rewrites and the shared Jev client
- [0017-plugins-bring-their-ui.md](../../../docs/decisions/0017-plugins-bring-their-ui.md)
  and [0030-host-halves-are-crates.md](../../../docs/decisions/0030-host-halves-are-crates.md):
  its UI and host half
