# tau-agent

A Rust library for running LLM agents inside your own program.

Agent workflows are plain async Rust. You start runs, `join!` them, fork
them from a checkpoint, and hand one agent to another as a tool. Every
run is stored in SQLite, so you can query transcripts and costs later.

```rust
use tau_agent::agent::Agent;
use tau_ai::{chatgpt::{self, ChatGpt}, client::OpenAi};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = tau_store_sqlite::open("runs.db").await?;
    // A saved Sign in with ChatGPT, with plan usage enabled.
    let chatgpt = ChatGpt::new(chatgpt::Store::open_default()?);
    let account = chatgpt.active()?.ok_or("sign in with ChatGPT first")?;
    let agent = Agent::new(OpenAi::chatgpt(chatgpt, account))
        .name("haiku")
        .instructions("Answer in a single haiku.");

    let outcome = agent.run("Why is the sky blue?", &store).await?;
    println!("{}\n(${:.4})", outcome.text, outcome.usage.cost.total);
    Ok(())
}
```

## What it is, and what it is not

- **A library.** It works on its own, and your program owns the control
  flow. tau, a desktop app with a phone build, is built on it: see
  [crates/tau/README.md](crates/tau/README.md) and
  [crates/tau-ui/README.md](crates/tau-ui/README.md).
- **OpenAI only.** It talks to the Responses API over its WebSocket mode
  (`wss://api.openai.com/v1/responses`), paid by the user's ChatGPT plan
  through Sign in with ChatGPT. There are no API keys.
  WebSocket is the only transport, so networks that block WebSocket
  upgrades will not work.
- **Embedded storage.** Runs, messages, forks and costs go to a SQLite
  file through `sqlx`, in `tau-store-sqlite`.
- **Coding tools are optional.** `read`, `bash`, `edit`, `write`, `grep`,
  `find` and `ls` live in the separate `tau-tools-host` crate.

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
tau-store-sqlite = { git = "https://github.com/cfcosta/tau-agent" }
tau-tools-host = { git = "https://github.com/cfcosta/tau-agent" } # optional

# For tests:
[dev-dependencies]
tau-testing = { git = "https://github.com/cfcosta/tau-agent" }
```

You also need a tokio runtime, and usually `serde`, `schemars` (typed
output and tools) and `async-trait` (custom tools and plugins).

The workspace uses Rust edition 2024 and pins a nightly toolchain. No
database is needed at build time: the sqlx query metadata is committed.

Each crate has its own `README.md`. The core crates are the library:

| Crate              | What it gives you                                                                        |
| ------------------ | ---------------------------------------------------------------------------------------- |
| `tau-agent`        | `Agent`, `Run`, the loop, tools, plugins, limits, typed output, forks, sub-agents        |
| `tau-ai`           | The `OpenAi` client, Sign in with ChatGPT, messages, models and pricing, the `Llm` trait |
| `tau-store`        | `Store`: storage for runs, transcripts and costs, behind a `Backend`                     |
| `tau-store-sqlite` | The SQLite `Backend`: `open` a database file, or `memory` for tests                      |
| `tau-artifacts`    | Private, quota-bound bytes that tools pass on by an opaque reference                     |
| `tau-testing`      | `ScriptedModel`, `block_on`, fake OpenAI and ChatGPT servers, generators                 |

The app and its interface, built on the library:

| Crate           | What it gives you                                                                 |
| --------------- | --------------------------------------------------------------------------------- |
| `tau`           | The desktop app (`cargo run -p tau`), with every plugin's host half               |
| `tau-ui`        | The host behind the interface: runs agents, signs in, lands, serves phones        |
| `tau-ui-remote` | The interface: the workspace, its screens, the plugins' views, the phone's remote |
| `tau-ui-plugin` | How a plugin brings its UI: `UiPlugin`, `HostHalf`, extension points, `Registry`  |
| `tau-ui-kit`    | The design language: theme tokens, icons, fonts and shared GPUI components        |
| `tau-phone`     | The interface on an Android phone, steering the tau on a computer                 |
| `tau-remote`    | What crosses between a phone and a computer: pairing, TLS identity, frames        |
| `tau-terminal`  | A terminal for tool output: libghostty-vt, a PTY runner and a GPUI view           |

Plugins, under `crates/plugins/`. Most come in two halves: one with the
cards and shapes every interface draws, which a phone links, and a
`-host` one with the behaviour. Light ones are one crate, some with a
`host` feature.

| Crate                   | What it gives you                                                     |
| ----------------------- | --------------------------------------------------------------------- |
| `tau-tools`             | The coding tools' cards, and the shapes they read                     |
| `tau-tools-host`        | Optional coding tools, all rooted at one directory                    |
| `tau-vcs`               | The version-control tools' cards, and the shapes they read            |
| `tau-vcs-host`          | Optional version-control tools on one jj workspace, backed by jj-lib  |
| `tau-codemode`          | Codemode's cards, and the records they fold                           |
| `tau-codemode-host`     | The `codemode` tool: a Luau script that calls the run's tools and Jev |
| `tau-memory`            | Long-term memory's note format, records and pages                     |
| `tau-memory-host`       | Long-term memory: a Zettelkasten of typed, linked Markdown notes      |
| `tau-constitution`      | A repository's rules, the checks' records and the Constitution page   |
| `tau-constitution-host` | Checks tool calls and final answers against the rules, with Jev       |
| `tau-mcp`               | The `mcpServers` format, the Servers page and MCP tools' cards        |
| `tau-mcp-host`          | Connects an agent to MCP servers and adds their tools                 |
| `tau-luau-plugins`      | Plugins written in Luau: what they declare, their records and views   |
| `tau-luau-plugins-host` | Loads Luau plugins and runs their hooks in codemode's sandbox         |
| `tau-compaction`        | Summarizing compaction, off unless you add it                         |
| `tau-fast-compaction`   | Prunes large tool outputs and stale tool history, with Jev            |
| `tau-tree-compaction`   | Folds old context into summaries to zoom into; off by default         |
| `tau-goal`              | Keeps a run going until a `/goal` holds, checked with Jev             |
| `tau-watcher`           | Every 6th step, rarely, a note on what you likely missed (off by default) |
| `tau-reasoning`         | Picks a run's reasoning effort from its task, with Jev                |
| `tau-ask`               | The `ask` tool: structured questions to the person                    |
| `tau-skills`            | Skills from `~/.agents/skills`, loaded with the `skill` tool          |
| `tau-direnv`            | Runs commands in the repository's direnv environment, once allowed    |
| `tau-jev`               | A client for Jev, TypeSafe's System One model, shared by plugins      |

Evaluations, under `crates/evals/`:

| Crate                     | What it gives you                                                 |
| ------------------------- | ----------------------------------------------------------------- |
| `tau-codemode-eval`       | Offline workloads and hand-written oracles for Codemode           |
| `tau-memory-e2e`          | The end-to-end evaluation of tau-memory; makes real model calls   |
| `tau-output-pruning-eval` | How well fast compaction's output pruning keeps what a task needs |

## Core concepts

- **`Agent`** describes what to run: model, instructions, tools, plugins
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
use tau_agent::{agent::Agent, limits::Limits};
use tau_compaction::Compaction;
use tau_ai::responses::request::ReasoningEffort;

// `llm` is an `OpenAi` client, made as in the first example.
let agent = Agent::new(llm.clone())
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
    .plugin(Compaction::default())     // off unless you add it
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
use tau_agent::tool::{typed, ToolCtx, ToolError, ToolOutput, TypedTool};

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

    async fn call(&self, args: WeatherArgs, _ctx: ToolCtx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(format!("Sunny in {}", args.city)))
    }
}

let agent = agent.tool(typed(Weather));
```

A tool that returns `Err` does not end the run: the error becomes the
tool result and the model sees it. `ToolError` converts with `?` from
io and JSON errors and from a `String` or `&str` message; wrap anything
else with `ToolError::other`. Use `ctx.updates.send(...)` to stream
partial output while a tool works, and `ctx.cancel` to stop early.

For hand-written schemas, implement `AgentTool` directly. Its
`execution_mode` can return `ExecutionMode::Sequential` to keep a batch
of calls from running in parallel.

### Plugins

A `Plugin` is shared by an agent's runs. For each run, `start` returns a
`PluginRun` that holds that run's state and sees every tool call and
every event. Plugins run in the order they were added, and each is
awaited.

```rust
use tau_agent::plugin::{
    Decision, Plugin, PluginCtx, PluginError, PluginRun, RunPlan, ToolCall,
};

struct NoRm;

#[async_trait]
impl Plugin for NoRm {
    fn name(&self) -> &str {
        "no-rm"
    }

    async fn start(&self, _plan: &mut RunPlan, _ctx: &PluginCtx) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(NoRmRun))
    }
}

struct NoRmRun;

#[async_trait]
impl PluginRun for NoRmRun {
    async fn before_tool(&mut self, call: &mut ToolCall, _ctx: &PluginCtx) -> Result<Decision, PluginError> {
        let command = call.args["command"].as_str().unwrap_or("");
        if call.name == "bash" && command.contains("rm -rf") {
            return Ok(Decision::Block("rm -rf is not allowed".into()));
        }
        Ok(Decision::Allow)
    }
}

let agent = agent.plugin(NoRm);
```

- `before_tool` can change the arguments (they are validated again) or
  block the call. A plugin that returns `Err` blocks the call too.
- `after_tool_result` can change the tool's output.
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

Long runs can outgrow the model's context window. The `tau-compaction`
plugin, added with `.plugin(Compaction::default())`, summarizes older
messages once the estimated context passes `context_window -
reserve_tokens`, and keeps roughly the last `keep_recent_tokens`
verbatim. It also compacts once if the model reports a context overflow.
Context plugins are offered the context in the order they were added,
so add compaction after any cheaper one, such as a pruner. A
`RunEvent::ContextRewritten` event marks each compaction.

`tau-fast-compaction` is a cheaper first step: it asks Jev, TypeSafe's
System One model, which tool calls and results still matter, and drops
or cuts the rest, keeping every text verbatim. Add it before
`Compaction`, which then takes only what pruning cannot free. See
[docs/reference/fast-compaction.md](docs/reference/fast-compaction.md).

`tau-tree-compaction` keeps what a summary would lose. It folds the
older messages into a history kept word for word, builds a binary tree
of one-line summaries over it after
[OptChat](https://gist.github.com/VictorTaelin/91837951a5ce5b38f341ec1ba1df6449),
and puts a view of the tree in their place. The `zoom` tool opens any
line down to a message whole. Add it before `Compaction`, which
summarizes when a line cannot be built. See
[docs/reference/tree-compaction.md](docs/reference/tree-compaction.md).

### Coding tools

`tau-tools-host` provides the seven coding tools from pi. Each is rooted at a
directory, and relative paths resolve against it.

```rust
use tau_tools_host::{path::Root, plugin::{CodingTools, Tool}};

let coder = Agent::new(llm.clone())
    .name("coder")
    .plugin(CodingTools::new(Root::new("/path/to/repo")));

// Or a subset: `.only(&[Tool::Read, Tool::Grep])`, `.without(Tool::Bash)`.
```

`CodingTools` is a plugin that only adds tools; `tau_tools_host::coding_tools`
returns the same seven for `Agent::tools`. `bash`, and so both, is
Unix-only. The search tools run in-process: no `rg` or `fd` binary is
needed.

The root sets where relative paths go. It is not a sandbox: absolute
paths and `bash` can reach the rest of the machine. Use a plugin to limit
what the tools may do.

### Testing your agents

`tau-testing` has `ScriptedModel`, an `Llm` that plays back scripted
turns and records what it was sent. Pair it with
`tau_store_sqlite::memory()` for fast, deterministic tests with no
network.

```rust
use serde_json::json;
use tau_testing::{block_on, scripted::ScriptedModel};

#[test]
fn looks_up_the_weather() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("weather", json!({"city": "Lisbon"})))
        .turn(|t| t.text("It's sunny in Lisbon."));

    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
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

The core crates and `tau-tools-host` are implemented and tested against a
scripted model and a simulated OpenAI server. Automated live tests against
the real endpoint are still pending (see [`docs/plan.md`](docs/plan.md));
`tau-ai`'s `chatgpt_probe` and `cache_probe` examples check it by hand.
The API may change before a first release, and no license has been chosen
yet.
