# tau-agent

Agents, runs and the agent loop. An `Agent` describes what to run: a
model, instructions, tools, plugins and limits. Each start makes a `Run`
that streams events, takes steering, can be cancelled, and is stored as it
goes. Workflows are plain async Rust on top of that: you `join!` runs, fork
one from a checkpoint, resume a finished one, or hand one agent to another
as a tool.

The loop, its event model, tool validation and the strict schema rewrite
are ported from pi (`packages/agent` and parts of `packages/ai`).
Compaction is not in this crate; it is a plugin, in `tau-compaction`.

## What it provides

| Module       | What it holds                                                                              |
| ------------ | ------------------------------------------------------------------------------------------ |
| `agent`      | `Agent` (the builder), `Run`, `RunControl`, `Outcome`, `Typed`, `Checkpoint`, `Input`      |
|              | `Forked` and `Resumed` (from `Agent::fork` and `Agent::resume`), `SubAgent` (`as_tool`)    |
| `tool`       | `AgentTool`, `TypedTool` and `typed`, `ToolCtx`, `ToolOutput`, `Exposure`, `ExecutionMode` |
| `plugin`     | `Plugin`, `PluginRun`, `RunPlan`, `PluginCtx`, `Decision`, `Rewrite`, `StopDecision`       |
| `event`      | `RunEvent`, `StopReason`, `LimitKind`                                                      |
| `limits`     | `Limits`: turns, tokens, USD, time and plugin continuations per run                        |
| `error`      | `ToolError` and `PluginError`, which convert with `?` from common errors                   |
| `runner`     | The loop for one run; `Clock` and `system_clock`                                           |
| `schema`     | `to_strict`: the OpenAI strict-mode rewrite of a JSON Schema                               |
| `validation` | `ArgumentSchema`: coerces and validates tool arguments before they run                     |
| `context`    | Token estimates for messages and transcripts, and `is_context_overflow`                    |
| `output`     | Cuts tool output too large for the model, and `Spill` keeps the whole of it in a file      |
| `launch`     | `Launch`, `Launcher`: how tools start programs, such as through a direnv environment       |

A run's events follow `RunStart (TurnStart … TurnEnd)* RunEnd`. Tool calls
are prepared in source order, then run together, or one at a time if any
tool is `Sequential`. Retryable model errors are retried inside the loop
(change the policy with `Agent::retry`). Limits are checked after every
turn, and a sub-agent's usage counts toward its caller's.

Plugins extend an agent from other crates. A `Plugin` adds tools and,
for each run, edits the `RunPlan` before the session opens and returns a
`PluginRun` with that run's state. Its hooks are `before_tool`,
`after_tool_result`, `on_event`, `before_request`, `rewrite_context`,
`rewritten`, `before_stop` and `finish`. Through `PluginCtx` a plugin asks
the model side questions, charges their cost to the run, and stores and
publishes records.

## How it fits

It builds on `tau-ai` (the `Llm` trait, messages, events, retry) and
`tau-store` (every run is written to a `Store`). It does not depend on
SQLite; pass it any `Store`, usually from `tau_store_sqlite::open`.

Every plugin crate builds on it, the interface halves (such as
`tau-tools`, `tau-vcs`, `tau-codemode`) for its types and the host halves
(such as `tau-tools-host`, `tau-vcs-host`, `tau-mcp-host`) for the
`Plugin` trait they implement. `tau-ui`, `tau-ui-plugin`, `tau-ui-remote`
and the evals run agents with it.

## Usage

```rust
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tau_agent::{
    agent::Agent,
    limits::Limits,
    tool::{ToolCtx, ToolError, ToolOutput, TypedTool, typed},
};

#[derive(Deserialize, JsonSchema)]
struct Args {
    /// The file to count.
    path: String,
}

struct LineCount;

#[async_trait]
impl TypedTool for LineCount {
    type Args = Args;
    const NAME: &'static str = "line_count";
    const DESCRIPTION: &'static str = "Counts the lines of a file.";

    async fn call(
        &self,
        args: Args,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let text = tokio::fs::read_to_string(&args.path).await?;
        Ok(ToolOutput::text(text.lines().count().to_string()))
    }
}

// `llm` is a `tau_ai::client::OpenAi`, or a `ScriptedModel` in tests.
let agent = Agent::new(llm)
    .name("counter")
    .instructions("Answer with a number.")
    .tool(typed(LineCount))
    .limits(Limits::default().max_turns(5).max_usd(0.50));

let store = tau_store_sqlite::open("runs.db").await?;
let outcome = agent.run("How long is Cargo.toml?", &store).await?;
println!("{} (${:.4})", outcome.text, outcome.usage.cost.total);

// Go on from where it ended, in a new run that inherits its transcript.
let fork = agent.fork(&outcome.checkpoint());
let followup = fork.run("And README.md?", &store).await?;
println!("{}", followup.text);
```

`Agent::start` returns a `Run` instead, whose `events()` stream the run as
it goes, and whose `steer` and `cancel` reach it while it works.
`Agent::run_typed::<T>` constrains the final message to `T`'s JSON schema
and parses it. `Agent::as_tool` makes an agent a tool for another one.

## Features

| Feature | What it does                                                                       |
| ------- | ---------------------------------------------------------------------------------- |
| `serde` | Serializes run events and tool outputs, for interfaces that carry them over a wire |

`tau-ui`, `tau-ui-plugin`, `tau-ui-remote`, `tau-codemode` and
`tau-codemode-host` turn it on.

## Testing

```sh
cargo nextest run --release -p tau-agent
```

The tests drive the loop with `tau_testing::ScriptedModel` and
`tau_store_sqlite::memory()`, under paused time. `tests/examples.rs` runs
the workflow examples of `docs/reference/api.md`. `tests/agent_loop.rs` is
a Hegel model of the loop over generated scripts, with a deeper variant
that is ignored by default:

```sh
cargo nextest run --release -p tau-agent --run-ignored only
```

`tests/compile.rs` checks with `trybuild` that misuse of the API does not
compile. The expected errors in `tests/ui/*.stderr` depend on the pinned
toolchain; after a toolchain update, regenerate them with
`TRYBUILD=overwrite` and review the diff. `tests/store_failure.rs` breaks
a database file mid-run, so it runs on a real runtime.

## Further reading

- [Public API](../../docs/reference/api.md)
- [Agent loop](../../docs/reference/agent-loop.md)
- [Plugins](../../docs/reference/plugins.md)
- [Compaction](../../docs/reference/compaction.md), for the token estimate
- [Agent commands' environment](../../docs/reference/environment.md), for `launch`
- [Testing](../../docs/reference/testing.md)
- [Decision 0001: A library, not a product](../../docs/decisions/0001-library-not-product.md)
- [Decision 0005: Plugins](../../docs/decisions/0005-plugins.md)
- [Decision 0015: Delegates fork their caller](../../docs/decisions/0015-delegates-fork-their-caller.md)
- [Decision 0026: Sub-agents run detached](../../docs/decisions/0026-sub-agents-run-detached.md)
- [Decision 0028: Async all the way](../../docs/decisions/0028-async-all-the-way-blocking-only-in-spawn-blocking.md)
