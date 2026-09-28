# tau-agent

A Rust library for running LLM agents inside your own program.

Agent workflows are plain async Rust. You start runs, `join!` them, fork
them from a checkpoint, and hand one agent to another as a tool. Every
run is stored in SQLite, so you can query transcripts and costs later.

```rust
use tau_agent::agent::Agent;
use tau_ai::client::OpenAi;
use tau_store::Store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let store = Store::open("runs.db").await?;
    let agent = Agent::new(OpenAi::from_env()?)
        .name("haiku")
        .instructions("Answer in a single haiku.");

    let outcome = agent.run("Why is the sky blue?", &store).await?;
    println!("{}\n(${:.4})", outcome.text, outcome.usage.cost.total);
    Ok(())
}
```

## What it is, and what it is not

- **A library.** There is no CLI, TUI or server. Your program owns the
  control flow.
- **OpenAI only.** It talks to the Responses API over its WebSocket mode
  (`wss://api.openai.com/v1/responses`), authenticated with an API key.
  WebSocket is the only transport, so networks that block WebSocket
  upgrades will not work.
- **Embedded storage.** Runs, messages, forks and costs go to a SQLite
  file through `sqlx`.
- **Coding tools are optional.** `read`, `bash`, `edit`, `write`, `grep`,
  `find` and `ls` live in the separate `tau-tools` crate.

The agent loop, event model, tool validation, compaction and WebSocket
continuation logic are ported from [pi](https://github.com/earendil-works/pi)
(audited at `2b0a123`, v0.87.1).

## Installation

The crates are not on crates.io yet. Add them as git dependencies:

```toml
[dependencies]
tau-agent = { git = "https://github.com/cfcosta/tau-agent" }
tau-ai    = { git = "https://github.com/cfcosta/tau-agent" }
tau-store = { git = "https://github.com/cfcosta/tau-agent" }
tau-tools = { git = "https://github.com/cfcosta/tau-agent" } # optional

# For tests:
[dev-dependencies]
tau-testing = { git = "https://github.com/cfcosta/tau-agent" }
```

You also need a tokio runtime, and usually `serde`, `schemars` (typed
output and tools), `async-trait` and `anyhow` (custom tools and hooks).

The workspace uses Rust edition 2024 and pins a nightly toolchain. No
database is needed at build time: the sqlx query metadata is committed.

| Crate         | What it gives you                                                               |
| ------------- | ------------------------------------------------------------------------------- |
| `tau-agent`   | `Agent`, `Run`, the loop, tools, hooks, limits, typed output, forks, sub-agents |
| `tau-ai`      | The `OpenAi` client, messages, models and pricing, the `Llm` trait              |
| `tau-store`   | `Store`: SQLite storage for runs, transcripts and costs                         |
| `tau-tools`   | Optional coding tools, all rooted at one directory                              |
| `tau-testing` | `ScriptedModel` and `block_on` for deterministic tests                          |

## Core concepts

- **`Agent`** describes what to run: model, instructions, tools, hooks
  and limits. It is cheap to clone and holds no state, so one agent can
  run many times, concurrently.
- **`Run`** is one execution of an agent. It streams events, accepts
  steering messages, and can be cancelled.
- **`Outcome`** is what a finished run returns: the final text, why it
  stopped, token usage and cost, and a checkpoint to fork from.
- **`Store`** records every run. Pass the same store to every run in a
  workflow.

## Guide

### Configuring an agent

`Agent` is a builder. Each method returns a new agent.

```rust
use std::time::Duration;
use tau_agent::{agent::Agent, compaction::Compaction, limits::Limits};
use tau_ai::{client::OpenAi, responses::request::ReasoningEffort};

let agent = Agent::new(OpenAi::from_env()?)
    .name("reviewer")              // shown in events and cost reports
    .model("gpt-5.5")              // the default
    .instructions("Review the diff. Be specific.")
    .reasoning(ReasoningEffort::High)
    .limits(
        Limits::default()
            .max_turns(20)
            .max_tokens(500_000)
            .max_usd(2.0)
            .timeout(Duration::from_secs(600)),
    )
    .compaction(Compaction::default()) // off unless you turn it on
    .warmup(true);                     // pre-send instructions and tools
```

`OpenAi` clients share one connection pool per clone, so create one and
clone it for every agent. It must be created inside a tokio runtime.

### Running

There are three ways to run an agent:

```rust
// Run to the end.
let outcome = agent.run("Summarize CHANGELOG.md", &store).await?;

// Run to the end and parse the final message as `T` (see below).
let typed = agent.run_typed::<Summary>("Summarize CHANGELOG.md", &store).await?;

// Start in the background and interact with the run.
let mut run = agent.start("Summarize CHANGELOG.md", &store);
```

`Outcome::stop` tells you why the run ended:

| `StopReason`  | Meaning                                                  |
| ------------- | -------------------------------------------------------- |
| `Stop`        | The model finished.                                      |
| `Limit(kind)` | A limit was hit: `Turns`, `Tokens`, `Usd` or `Time`.     |
| `Cancelled`   | `Run::cancel` was called, or a parent run was cancelled. |
| `Error(msg)`  | The model returned an error that retries did not fix.    |

Retryable model errors are retried inside the loop (3 attempts with a 2 s
base by default; change it with `Agent::retry`). Dropped WebSocket
continuations and connection-limit errors are recovered transparently.

### Streaming events, steering and cancelling

```rust
use futures_util::StreamExt;
use tau_agent::event::RunEvent;

let mut run = agent.start("Refactor the retry module.", &store);
{
    let mut events = run.events();
    while let Some(event) = events.next().await {
        match event {
            RunEvent::TextDelta { delta, .. } => print!("{delta}"),
            RunEvent::ToolStart { tool, args, .. } => eprintln!("-> {tool} {args}"),
            RunEvent::RunEnd { stop, cost, .. } => eprintln!("\n{stop:?} ${cost:.4}"),
            _ => {}
        }
    }
}
let outcome = run.outcome().await?;
```

While a run is going:

- `run.steer("Also update the docs.")` queues a user message. It is added
  after the current batch of tool calls.
- `run.cancel()` stops the run. Running tools see the cancellation
  through `ToolCtx::cancel`.

`run.outcome()` drops events you have not read, so a run nobody listens
to never blocks on its event channel.

### Typed output

`run_typed::<T>` sends `T`'s JSON schema as a strict structured-output
format, and parses the final message as `T`. If the message does not
parse, you get `AgentError::Output`, which still carries the outcome.

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct Review {
    approved: bool,
    problems: Vec<String>,
}

let review = reviewer.run_typed::<Review>(diff, &store).await?;
if !review.value.approved {
    // `review.json()` serializes the value, to pass on to another agent.
    fixer.run(review.json(), &store).await?;
}
```

### Tools

Implement `TypedTool` and wrap it with `typed`. The argument schema is
generated from `Args` with `schemars`, rewritten to OpenAI's strict form,
and arguments are validated (with small repairs, such as `"3"` → `3`)
before your code sees them.

```rust
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tau_agent::tool::{typed, ToolCtx, ToolOutput, TypedTool};

#[derive(Deserialize, JsonSchema)]
struct WeatherArgs {
    /// City name, e.g. "Lisbon".
    city: String,
}

struct Weather;

#[async_trait]
impl TypedTool for Weather {
    type Args = WeatherArgs;
    const NAME: &'static str = "weather";
    const DESCRIPTION: &'static str = "Current weather for a city.";

    async fn call(&self, args: WeatherArgs, _ctx: ToolCtx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::text(format!("Sunny in {}", args.city)))
    }
}

let agent = agent.tool(typed(Weather));
```

A tool that returns `Err` does not end the run: the error becomes the
tool result and the model sees it. Use `ctx.updates.send(...)` to stream
partial output while a tool works, and `ctx.cancel` to stop early.

For hand-written schemas, implement `AgentTool` directly. Its
`execution_mode` can return `ExecutionMode::Sequential` to keep a batch
of calls from running in parallel.

### Hooks

A `RunHook` sees every tool call and every event. Hooks run in the order
they were added, and each is awaited.

```rust
use tau_agent::hook::{Decision, HookCtx, RunHook, ToolCall};

struct NoRm;

#[async_trait]
impl RunHook for NoRm {
    async fn before_tool(&self, call: &mut ToolCall, _ctx: &HookCtx) -> anyhow::Result<Decision> {
        let command = call.args["command"].as_str().unwrap_or("");
        if call.name == "bash" && command.contains("rm -rf") {
            return Ok(Decision::Block("rm -rf is not allowed".into()));
        }
        Ok(Decision::Allow)
    }
}

let agent = agent.hook(NoRm);
```

- `before_tool` can change the arguments (they are validated again) or
  block the call. A hook that returns `Err` blocks the call too.
- `after_tool` can change the tool's output.
- `on_event` sees each `RunEvent` in order.

### Sub-agents

`Agent::as_tool` turns an agent into a tool that another agent can call.
Each call starts a child run. Cancelling the parent cancels the child,
the child's cost counts toward the parent's limits and outcome, and the
child's events reach the parent's event stream (with `parent` set).

```rust
let lead = Agent::new(llm.clone())
    .name("lead")
    .instructions("Break the task down. Delegate. Verify before finishing.")
    .tool(researcher.as_tool("research", "Investigate a question and report findings."))
    .tool(coder.as_tool("implement", "Make a scoped code change and report what changed."))
    .limits(Limits::default().max_usd(5.0));
```

The model calls a sub-agent tool with `{"input": "..."}`.

### Forks

Every outcome has a checkpoint. `Agent::fork` starts new runs that
continue from it, with the whole transcript up to that point. Forks run
concurrently and are stored as forks of the original run.

```rust
let investigation = debugger.run("Find the root cause. Don't fix it yet.", &store).await?;
let base = investigation.checkpoint();

let attempts = ["minimal fix", "fix plus regression test", "refactor clock injection"]
    .map(|approach| debugger.fork(&base).run(format!("Now implement: {approach}"), &store));
let results = futures_util::future::join_all(attempts).await;
```

### Workflows and cost

Tag runs with a workflow id through `Input`, then ask the store what the
workflow cost, per agent:

```rust
use tau_agent::agent::Input;

let changes = scanner
    .run_typed::<Changes>(Input::new("v1.4.0..v1.5.0").workflow("release"), &store)
    .await?;
// ... more runs with .workflow("release") ...

for row in store.workflow_cost("release").await? {
    println!("{}: {} runs, ${:.4}", row.agent, row.runs, row.usd);
}
```

`store.run(id)` returns a run's record (status, tokens, cost, result),
and `store.transcript(id)` returns its messages.

### Compaction

Long runs can outgrow the model's context window. With
`.compaction(Compaction::default())`, the loop summarizes older messages
once the estimated context passes `context_window - reserve_tokens`, and
keeps roughly the last `keep_recent_tokens` verbatim. It also compacts
once if the model reports a context overflow. Compaction is a plugin
that runs after any other context plugin, and a
`RunEvent::ContextRewritten` event marks each compaction.

### Coding tools

`tau-tools` provides the seven coding tools from pi. Each is rooted at a
directory, and relative paths resolve against it.

```rust
use tau_tools::{coding_tools, path::Root};

let coder = Agent::new(OpenAi::from_env()?)
    .name("coder")
    .tools(coding_tools(&Root::new("/path/to/repo")));
```

`bash`, and so `coding_tools`, is Unix-only. The search tools run
in-process: no `rg` or `fd` binary is needed. To pick a subset, build
them one by one, for example `tau_tools::read::Read::new(root.clone())`.

The root sets where relative paths go. It is not a sandbox: absolute
paths and `bash` can reach the rest of the machine. Use a hook to limit
what the tools may do.

### Testing your agents

`tau-testing` has `ScriptedModel`, an `Llm` that plays back scripted
turns and records what it was sent. Pair it with `Store::memory()` for
fast, deterministic tests with no network.

```rust
use serde_json::json;
use tau_testing::{block_on, scripted::ScriptedModel};

#[test]
fn looks_up_the_weather() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("weather", json!({"city": "Lisbon"})))
        .turn(|t| t.text("It's sunny in Lisbon."));

    block_on(async {
        let store = Store::memory().await.unwrap();
        let outcome = Agent::new(llm.clone())
            .tool(typed(Weather))
            .run("Weather in Lisbon?", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "It's sunny in Lisbon.");
        assert_eq!(llm.requests().len(), 2);
    });
}
```

## Further reading

The [integration examples](crates/tau-agent/tests/examples.rs) are three
complete workflows: a typed pipeline, a supervisor with sub-agents, and
a fork fan-out.

| Document                                                                   | What it covers                                        |
| -------------------------------------------------------------------------- | ----------------------------------------------------- |
| [`docs/reference/api.md`](docs/reference/api.md)                           | The public API in depth                               |
| [`docs/reference/agent-loop.md`](docs/reference/agent-loop.md)             | Loop semantics and ordering guarantees                |
| [`docs/reference/tools.md`](docs/reference/tools.md)                       | Exact behaviour and error strings of the coding tools |
| [`docs/reference/compaction.md`](docs/reference/compaction.md)             | When and how long runs are summarized                 |
| [`docs/reference/openai-websocket.md`](docs/reference/openai-websocket.md) | Connection pooling, continuation and recovery         |
| [`docs/reference/storage.md`](docs/reference/storage.md)                   | The SQLite schema                                     |
| [`docs/architecture.md`](docs/architecture.md)                             | How the crates fit together                           |
| [`docs/plan.md`](docs/plan.md)                                             | Project status and milestones                         |
| [`docs/decisions/`](docs/decisions/)                                       | Why the fixed decisions were made                     |

## Status

The core crates and `tau-tools` are implemented and tested against a
scripted model and a simulated OpenAI server. Live tests against the real
endpoint are still pending (see [`docs/plan.md`](docs/plan.md)). The API
may change before a first release, and no license has been chosen yet.
