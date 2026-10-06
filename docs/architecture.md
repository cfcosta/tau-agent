# Architecture

## Crates

```
tau-agent/
├── crates/
│   ├── tau/          # the desktop app: tau-ui with every plugin's host half,
│   │                 # the Luau ones included (ADR 0030)
│   ├── tau-ai/       # types, events, Responses conversion, WebSocket pool, retry, cost
│   ├── tau-agent/    # Agent, Run, loop, AgentTool + Plugin, typed results,
│   │                 # limits, sub-agents, forks, context estimate
│   ├── tau-store/    # runs, messages, fork transcripts: the types and the
│   │                 # Backend a Store is, with no database driver
│   ├── tau-store-sqlite/ # the Backend on SQLite via sqlx, with migrations
│   ├── tau-testing/  # ScriptedModel, recorded-stream replay
│   ├── tau-ui-kit/   # the design language: theme tokens, icons, fonts, components,
│   │                 # text field, marked-up text (ADR 0017)
│   ├── tau-ui-plugin/ # UiPlugin, HostHalf, its Fold, the registry, extension
│   │                 # points (ADR 0017, 0030)
│   ├── tau-ui-remote/ # the interface: workspace, screens, plugins' views, and the
│   │                 # phone's remote that drives it from a computer (ADR 0013)
│   ├── tau-ui/       # the computer's side: the host that runs agents, the
│   │                 # phone server, onboarding's sign-ins, the demo
│   ├── tau-phone/    # tau-ui-remote on Android
│   ├── tau-terminal/ # libghostty-vt terminal, PTY command runner,
│   │                 # plain text, styled snapshots, GPUI TerminalView; no tau deps
│   └── plugins/                # each plugin's interface half, and its
│       │                         # host half: `tau-<plugin>-host` when heavy
│       │                         # (ADR 0030)
│       ├── tau-ask/              # the agent asks the person; the panel in the
│       │                         # composer's place (ADR 0019)
│       ├── tau-codemode/         # Luau scripts that call tools and Jev
│       ├── tau-compaction/       # summarizing compaction
│       ├── tau-constitution/     # a repository's rules, checked with Jev
│       ├── tau-direnv/           # agent commands in the repository's
│       │                         # direnv environment (ADR 0025)
│       ├── tau-fast-compaction/  # Jev-driven pruning of tool history
│       ├── tau-goal/             # keeps a chat going until its /goal holds
│       ├── tau-jev/              # TypeSafe's Jev client, for plugins
│       ├── tau-luau-plugins/     # plugins written in Luau, run in codemode's
│       │                         # sandbox (ADR 0027)
│       ├── tau-mcp/              # MCP servers' tools (rmcp), the Servers page
│       ├── tau-memory/           # linked notes each repository's runs keep
│       ├── tau-reasoning/        # picks a run's reasoning effort
│       ├── tau-skills/           # instructions an agent loads when a task
│       │                         # calls for them
│       ├── tau-tools/            # read, bash, edit, write, grep, find, ls
│       └── tau-vcs/              # version control on jj-lib, and landing runs
└── docs/
```

Dependency direction:

- `tau-agent` depends on `tau-ai` and `tau-store`, which has no
  database driver: the agent loop and the plugins build while
  `tau-store-sqlite` compiles SQLite. Whoever opens a store depends
  on `tau-store-sqlite`: `tau-ui`, the evaluations and tests.
- `tau-testing` depends on `tau-ai`.
- `tau-ui-kit` depends on GPUI and `tau-terminal` (the theme's terminal
  palette), and on no other tau crate.
- `tau-terminal` depends on no tau crate. `tau-tools-host` uses it to
  run `bash` under a pseudo-terminal, and `tau-tools` to draw `bash`
  cards, each only with its `terminal` feature, which `tau-ui` turns on
  ([0010](decisions/0010-terminal-rendering.md)).
- `tau-ui-remote` draws runs and starts none: it depends on the
  plugins' crates, not on their host halves, and lists the plugins
  without them. `tau-ui` brings the host halves (the `tau-*-host`
  crates, and the `host` feature of the plugins that keep theirs),
  gives each plugin its own, and drives it; `tau-phone` depends on
  `tau-ui-remote` alone ([0030](decisions/0030-host-halves-are-crates.md)).
- `tau`, the app, is `tau-ui` with the Luau plugins' host halves
  (tau-codemode's and tau-luau-plugins'), which only it links: `tau-ui`
  builds while Luau's C++ does.
- Plugins, under `crates/plugins/`, depend on `tau-agent` (and
  `tau-ai` for message types). Core crates never depend on a plugin
  ([0006](decisions/0006-plugin-crates.md)). Each plugin brings its own
  UI through `tau-ui-plugin`, and has no headless form
  ([0017](decisions/0017-plugins-bring-their-ui.md)). Its agent half
  publishes records of one enum, and its state folds them (`Fold`), live
  and from history alike.

## Data flow for one turn

```
Agent::start(input, &store)
  └─ Run task
       ├─ store: INSERT runs (status = running)
       ├─ lane = pool.open_lane(affinity)            # the conversation's socket
       └─ loop
            ├─ transcript: kept in memory; fork ancestors loaded once at start
            ├─ body = session fields (built once) + InputCache(transcript)
            ├─ lane.create(body)                     # delta rule: suffix + previous_response_id
            │    └─ response.* events → AssistantEvent stream → RunEvent (plugins awaited)
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
  - Each run gets a lane on a connection of its own, with one response
    in flight. tau sends no `stream_id`, which the plan route may not
    take. Requests on a lane are FIFO; different lanes run concurrently.
  - The prompt cache lives on the connection, so a lane takes one that
    already serves its conversation, or for a fork's first request its
    parent's, before any other free one or a new one
    ([0022](decisions/0022-the-prompt-cache-follows-the-connection.md),
    `docs/reference/openai-websocket.md`, "Connection pool").
- **Nothing blocks outside `spawn_blocking`**
  ([0028](decisions/0028-async-all-the-way-blocking-only-in-spawn-blocking.md)).
  GPUI's thread only draws and sends requests; the host answers on its
  tokio runtime and pushes what changed. jj-lib runs in
  `spawn_blocking`, one job at a time per workspace or repository,
  under an async lock.
- **Each run is a tokio task.** A run owns:
  - a `CancellationToken`;
  - a bounded event channel;
  - a steering queue.
- **Tools run in parallel within a batch** (`join_all`).
  - Tool futures are not dropped to cancel them. Each tool receives the
    run's token and observes it itself.
  - Only provider streams are dropped inside `select!`.
- **Plugins and event subscribers are awaited in order.** A slow
  subscriber applies backpressure to its run and to no other run.
- **MCP servers belong to the host, not to a run.** tau-mcp keeps one
  plugin per repository on the host, whose connections every run in the
  repository shares; they close when the host goes
  ([mcp.md](reference/mcp.md)).
- **Store access:**
  - all writes go through one writer connection;
  - reads use a read-only pool;
  - each turn costs one write transaction.

## I/O boundary

The protocol logic in `tau-ai` does no I/O. It is written as plain state
machines that take events and return actions:

- **`ws::proto`** holds the lane and pool state: the delta rule, the
  continuation per lane and per connection, one lane per connection,
  which connection a lane takes (conversation affinity), connection
  age, and the recovery ladder. Its inputs are events such as "request
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
| `beforeToolCall` / `afterToolCall` / listeners     | `PluginRun` (`before_tool` / `after_tool_result` / `on_event`)     |
| steering queue                                     | `Run::steer`                                                       |
| follow-up queue                                    | not ported (start another run)                                     |
| JSONL session tree                                 | `runs` + `messages` tables; forks via `parent_run_id` / `fork_seq` |
| compaction (`coding-agent/src/core/compaction`)    | `tau-compaction` (a plugin)                                        |
| faux provider                                      | `tau-testing::ScriptedModel`                                       |
| built-in tools                                     | `tau-tools-host`                                                   |
