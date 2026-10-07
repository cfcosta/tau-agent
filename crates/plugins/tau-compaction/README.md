# tau-compaction

Summarizing compaction for tau agents, as a plugin. When a run's context
nears the model's window, it asks the model for a structured summary of
the older messages and replaces them with it, keeping the recent ones
verbatim. It is a port of pi's coding-agent compaction. An agent
compacts only when it has this plugin; compaction is off by default.

## What it provides

The crate root holds the rules as pure functions and plain data: no
store, no LLM request, no async. The plugin builds on them.

- `Compaction`: the settings (`reserve_tokens`, `keep_recent_tokens`,
  `context_window`, each with a builder method) and the plugin itself.
- `should_compact`: whether the estimate has passed
  `context_window - reserve_tokens`.
- `find_cut_point` and `CutPoint`: where to cut so the trailing tokens
  stay verbatim, never between a tool call and its result.
- `plan` and `Plan`: which messages a pass summarizes, which split-turn
  prefix gets its own summary, and the first message kept.
- `serialize_conversation`: the transcript as flat text for the summary
  request, with tool results cut at 2,000 characters.
- `build_summary_request`, `build_turn_prefix_summary_request` and
  `merge_split_turn_summary`: the summary requests, and joining a split
  turn's two summaries.
- `FileOperations` and `format_file_operations`: the files read and
  modified, carried from one summary to the next.
- `check_summary` and `CompactionError`: reject a response that errored,
  hit the token cap or called a tool.
- `Record`: what a compaction stores, the summary, the tokens before and
  the file lists.
- `SUMMARIZATION_SYSTEM_PROMPT`, `SUMMARIZATION_PROMPT`,
  `UPDATE_SUMMARIZATION_PROMPT`, `TURN_PREFIX_SUMMARIZATION_PROMPT`,
  `SUMMARY_PREFIX` and `SUMMARY_SUFFIX`: pi's prompts and wrapper, the
  prompts with `PRIORITIES`, OptChat's ranking of what a summary keeps
  (`docs/reference/compaction.md`, "What the prompts add").
- `NAME`: `"tau-compaction"`, its name in events and stored rewrites.
- `ui::CompactionUi` and `ui::CompactionHost`: its UI and its host half.

`Compaction::default()` reserves 16,384 tokens and keeps the last 20,000
verbatim. The plugin compacts after a turn once the estimate passes the
threshold, before the first request of a run on an inherited transcript,
and once on a context overflow, after which the loop retries once. A
rejected summary writes nothing. Past the threshold the run goes on and
compaction waits 2, 4, 8 and up to 32 turns before trying again; on an
overflow the failure fails the run.

## How it fits

It is a single crate: the rules, the plugin, its `UiPlugin` and its
`HostHalf` all live here. It builds on `tau-agent` (the `Plugin` and
`PluginRun` seams, `PluginCtx::ask`, the loop's token estimate in
`tau_agent::context`) and `tau-ai` (messages and the model registry).
Its UI uses `tau-ui-kit` and `tau-ui-plugin`.

`tau-ui` registers `CompactionHost` and `tau-ui-remote` registers
`CompactionUi`. The host half adds `Compaction` to every run, on the
window of the run's own model. `tau-memory-e2e` adds it to each arm's
agent, as the app does. `tau-fast-compaction` uses it in tests only.

## Usage

Add it after every other context plugin. Context plugins are offered
the context in the order they were added, so cheaper rewrites such as
pruning get the first chance.

```rust
use tau_agent::agent::Agent;
use tau_compaction::Compaction;

let agent = Agent::new(llm)
    .instructions("You are a careful coding agent.")
    .plugin(Compaction::default().keep_recent_tokens(30_000));
```

For a model the registry does not know, set the window by hand with
`Compaction::default().context_window(200_000)`. Without a known window,
only an overflow compacts.

## Testing

```sh
cargo nextest run --release -p tau-compaction
```

`tests/rules.rs` holds Hegel property tests of the token estimate, the
threshold and the cut point, plus pi's known cases. `tests/in_the_loop.rs`
runs the plugin through the agent loop with `tau_testing`'s
`ScriptedModel` and `tau_store_sqlite::memory()`: threshold and overflow
compaction, split turns, rejected summaries and their backoff, and
forks of a compacted run. No test needs credentials or the network.

## Further reading

- [compaction.md](../../../docs/reference/compaction.md): the rules
  this crate implements, and where it differs from pi
- [plugins.md](../../../docs/reference/plugins.md): the plugin seams
  and context rewrites
- [0005-plugins.md](../../../docs/decisions/0005-plugins.md) and
  [0006-plugin-crates.md](../../../docs/decisions/0006-plugin-crates.md):
  why compaction became a plugin in its own crate
- [0017-plugins-bring-their-ui.md](../../../docs/decisions/0017-plugins-bring-their-ui.md)
  and [0030-host-halves-are-crates.md](../../../docs/decisions/0030-host-halves-are-crates.md):
  its UI and host half
