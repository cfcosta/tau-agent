# Architecture

## Crates

```
tau-agent/
├── crates/
│   ├── tau-ai/       # types, events, Responses conversion, WebSocket pool, retry, cost
│   ├── tau-agent/    # Agent, Run, loop, AgentTool + RunHook, plugins, typed results,
│   │                 # limits, sub-agents, forks, context estimate
│   ├── tau-store/    # SQLite via sqlx: runs, messages, fork transcripts, migrations
│   ├── tau-testing/  # ScriptedModel, recorded-stream replay
│   ├── tau-ui-kit/   # the design language: theme tokens, icons, fonts, components,
│   │                 # text field, marked-up text (ADR 0017)
│   ├── terminal/     # tau-terminal: libghostty-vt terminal, PTY command runner,
│   │                 # plain text, styled snapshots, GPUI TerminalView; no tau deps
│   └── plugins/
│       ├── compaction/       # tau-compaction: summarizing compaction
│       ├── fast-compaction/  # tau-fast-compaction: Jev-driven pruning of tool history
│       ├── jev/              # tau-jev: TypeSafe's Jev client, for plugins
│       └── tools/            # tau-tools: read, bash, edit, write, grep, find, ls
└── docs/
```

Dependency direction:

- `tau-agent` depends on `tau-ai` and `tau-store`.
- `tau-testing` depends on `tau-ai`.
- `tau-ui-kit` depends on GPUI and `tau-terminal` (the theme's terminal
  palette), and on no other tau crate.
- `tau-terminal` depends on no tau crate. `tau-tools` uses it for
  `bash` only with its `terminal` feature, which `tau-ui` turns on
  ([0010](decisions/0010-terminal-rendering.md)).
- Plugins, under `crates/plugins/`, depend on `tau-agent` (and
  `tau-ai` for message types). Core crates never depend on a plugin
  ([0006](decisions/0006-plugin-crates.md)). Each plugin brings its own
  UI through `tau-ui-plugin`, and has no headless form
  ([0017](decisions/0017-plugins-bring-their-ui.md); not built yet).

## Data flow for one turn

```
Agent::start(input, &store)
  └─ Run task
       ├─ store: INSERT runs (status = running)
       ├─ lane = pool.acquire_lane()                 # a socket of its own
       └─ loop
            ├─ transcript: kept in memory; fork ancestors loaded once at start
            ├─ body = session fields (built once) + InputCache(transcript)
            ├─ lane.create(body)                     # delta rule: suffix + previous_response_id
            │    └─ response.* events → AssistantEvent stream → RunEvent (hooks awaited)
            ├─ tool calls? prepare sequentially → execute in parallel → results in source order
            ├─ store.append_turn(messages, usage)    # one write transaction
            ├─ limits / cancel / steering checks
            └─ compaction threshold? summarize → compaction record → next turn is a full resend
       └─ store: UPDATE runs (status, result)
```

## Concurrency model

- **One `OpenAi` client per process.** It reaches OpenAI through a
  ChatGPT sign-in, the only way in
  ([0012](decisions/0012-chatgpt-sign-in-only.md)), and owns a pool of
  WebSocket connections.
  - Each run gets a lane: a connection of its own, with one response
    in flight. tau sends no `stream_id`, which the plan route may not
    take. Requests on a lane are FIFO; different lanes run concurrently.
  - A new lane takes an open connection that has no lane and is not
    draining, or the pool opens one.
- **Each run is a tokio task.** A run owns:
  - a `CancellationToken`;
  - a bounded event channel;
  - a steering queue.
- **Tools run in parallel within a batch** (`join_all`).
  - Tool futures are not dropped to cancel them. Each tool receives the
    run's token and observes it itself.
  - Only provider streams are dropped inside `select!`.
- **Hooks and event subscribers are awaited in order.** A slow
  subscriber applies backpressure to its run and to no other run.
- **Store access:**
  - all writes go through one writer connection;
  - reads use a read-only pool;
  - each turn costs one write transaction.

## I/O boundary

The protocol logic in `tau-ai` does no I/O. It is written as plain state
machines that take events and return actions:

- **`ws::proto`** holds the lane and pool state: the delta rule, the
  continuation per lane, one lane per connection, connection age,
  and the recovery ladder. Its inputs are events such as "request
  submitted", "frame received", "connection closed", "timer fired" and
  "cancel". Its outputs are actions such as "send this frame", "open a
  connection", "close this connection", "emit this event" and "set this
  timer". It never reads a clock; the current time is part of each
  input.
- **`responses`** turns transcripts into input items, and `response.*`
  frames into `AssistantEvent`s. It is pure too.
- **`ws::io`** is a thin driver. One tokio task per connection owns that
  connection's state and runs the loop: read a frame or a command, feed
  it to `ws::proto`, carry out the actions. Runs talk to it over
  channels, so no lock is shared between lanes.
- **`Connector`** is the trait the driver uses to open a socket. The
  product one, `ChatGptConnector`, gets the plan's token and opens TLS
  to `wss://api.openai.com`. Tests pass a connector
  that opens a stream on a simulated network.

This split is what makes the WebSocket layer testable: the rules that
are hard to get right are checked without a network, and the part that
touches a network is small enough to test in simulation. See
[`reference/testing.md`](reference/testing.md).

## Why the WebSocket shapes the design

The Responses WebSocket keeps the previous response in connection memory,
so a turn only needs to send new input. That holds only while the rest of
the request stays identical. So:

- **Instructions, tools and reasoning settings are fixed per run.**
  `Agent` values are immutable once built.
- **A fork starts a new chain.** Its first turn is a full resend of the
  inherited transcript.
- **Compaction breaks the chain on purpose.** The turn after it is a full
  resend.
- **Cancellation keeps or drops the chain explicitly.** It closes or
  resets the lane's continuation; it never leaves the chain stale.

## Mapping from pi

| pi (TypeScript)                                    | tau-agent (Rust)                                                   |
| -------------------------------------------------- | ------------------------------------------------------------------ |
| `pi-ai` `AssistantMessageEvent` + shared `partial` | `tau-ai::AssistantEvent` (owned deltas) + `Accumulator`            |
| `openai-responses-shared.ts`                       | `tau-ai::responses`                                                |
| `openai-codex-responses.ts` WebSocket cache        | `tau-ai::ws` (plan endpoint, one lane per connection)              |
| `pi-agent-core` `Agent` / `agentLoop`              | `tau-agent::Agent` / `Run`                                         |
| `AgentTool.execute(id, params, signal, onUpdate)`  | `AgentTool::call(args, ToolCtx { cancel, updates })`               |
| `beforeToolCall` / `afterToolCall` / listeners     | `RunHook`                                                          |
| steering queue                                     | `Run::steer`                                                       |
| follow-up queue                                    | not ported (start another run)                                     |
| JSONL session tree                                 | `runs` + `messages` tables; forks via `parent_run_id` / `fork_seq` |
| compaction (`coding-agent/src/core/compaction`)    | `tau-agent::compaction`                                            |
| faux provider                                      | `tau-testing::ScriptedModel`                                       |
| built-in tools                                     | `tau-tools`                                                        |
