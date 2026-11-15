# Architecture

## Crates

```
heiko-agent/
├── crates/
│   ├── heiko/          # the desktop app: heiko-ui with every plugin's host half,
│   │                 # the Luau ones included (ADR 0030)
│   ├── heiko-ai/       # types, events, Responses conversion, WebSocket pool, retry, cost
│   ├── heiko-agent/    # Agent, Run, loop, AgentTool + Plugin, typed results,
│   │                 # limits, sub-agents, forks, context estimate
│   ├── heiko-clock/    # the host's clock and local days: agents' schedules,
│   │                 # Luau plugins' local time (ADR 0037, 0050)
│   ├── heiko-perf/     # where the time goes: timings, and stalls of the
│   │                 # interface's thread (docs/reference/performance.md)
│   ├── heiko-store/    # runs, messages, fork transcripts: the types and the
│   │                 # Backend a Store is, with no database driver
│   ├── heiko-store-sqlite/ # the Backend on SQLite via sqlx, with migrations
│   ├── heiko-testing/  # ScriptedModel, recorded-stream replay
│   ├── heiko-ui-kit/   # the design language: theme tokens, icons, fonts, components,
│   │                 # text field, marked-up text (ADR 0017)
│   ├── heiko-ui-plugin/ # UiPlugin, HostHalf, its Fold, the registry, extension
│   │                 # points (ADR 0017, 0030)
│   ├── heiko-ui-remote/ # the interface: workspace, screens, plugins' views, and the
│   │                 # phone's remote that drives it from a computer (ADR 0013)
│   ├── heiko-ui/       # the computer's side: the host that runs agents, the
│   │                 # phone server, onboarding's sign-ins, the demo
│   ├── heiko-phone/    # heiko-ui-remote on Android
│   ├── heiko-terminal/ # libghostty-vt terminal, PTY command runner,
│   │                 # plain text, styled snapshots, GPUI TerminalView; no heiko deps
│   └── plugins/                # each plugin's interface half, and its
│       │                         # host half: `heiko-<plugin>-host` when heavy
│       │                         # (ADR 0030)
│       ├── heiko-ask/              # the agent asks the person; the panel in the
│       │                         # composer's place (ADR 0019; Luau)
│       ├── heiko-chatgpt/          # the person's ChatGPT plan, filling
│       │                         # `heiko.provider`
│       ├── heiko-codemode/         # Luau scripts that call tools and Jev
│       ├── heiko-compaction/       # summarizing compaction (Luau)
│       ├── heiko-compaction-chain/ # pruning, the tree and the summary in
│       │                         # turn, filling `heiko.compaction` (Luau)
│       ├── heiko-constitution/     # a repository's rules, checked with Jev (Luau)
│       ├── heiko-direnv/           # agent commands in the repository's
│       │                         # direnv environment (ADR 0025)
│       ├── heiko-fast-compaction/  # Jev-driven pruning of tool history (Luau)
│       ├── heiko-goal/             # keeps a chat going until its /goal holds (Luau)
│       ├── heiko-jev/              # TypeSafe's Jev client, and the plugin that
│       │                         # keeps its key and gives it to plugins
│       ├── heiko-luau-plugins/     # plugins written in Luau, run in codemode's
│       │                         # sandbox (ADR 0027)
│       ├── heiko-mcp/              # MCP servers' tools (rmcp), the Servers page
│       ├── heiko-memory/           # linked notes each repository's runs keep (Luau)
│       ├── heiko-notify/           # desktop notices while heiko is in the
│       │                         # background, filling `heiko.notify` (Luau)
│       ├── heiko-reasoning/        # picks a run's reasoning effort on auto (Luau)
│       ├── heiko-sqlite/           # the store in SQLite, filling `heiko.store`
│       ├── heiko-titles/           # a run's title from the person's model,
│       │                         # filling `heiko.title` (Luau)
│       ├── heiko-tools/            # read, exec_command, write_stdin, edit, ...
│       ├── heiko-tree-compaction/  # compaction into a tree of summaries the
│       │                         # agent zooms into, after OptChat (Luau)
│       ├── heiko-vcs/              # version control on jj-lib, and landing runs
│       └── heiko-watcher/          # notes what you likely missed, every 6th step (Luau)
│                                 # (ADR 0032)
└── docs/
```

Dependency direction:

- `heiko-agent` depends on `heiko-ai` and `heiko-store`, which has no
  database driver: the agent loop and the plugins build while
  `heiko-store-sqlite` compiles SQLite. Whoever opens a store depends
  on `heiko-store-sqlite`: the heiko-sqlite plugin, which fills `heiko.store`
  for the host, the evaluations and tests.
- `heiko-testing` depends on `heiko-ai`.
- `heiko-ui-kit` depends on GPUI and `heiko-terminal` (the theme's terminal
  palette), and on no other heiko crate.
- `heiko-terminal` depends on no heiko crate. `heiko-tools-host` uses it to
  run commands under a pseudo-terminal, and `heiko-tools` to draw their
  cards, the latter only with its `terminal` feature, which `heiko-ui`
  turns on ([0010](decisions/0010-terminal-rendering.md),
  [0038](decisions/0038-commands-run-in-the-background.md)).
- `heiko-ui-remote` draws runs and starts none: it depends on the
  plugins' crates, not on their host halves, and lists the plugins
  without them. `heiko-ui` brings the host halves (the `heiko-*-host`
  crates, and the `host` feature of the plugins that keep theirs),
  gives each plugin its own, and drives it; `heiko-phone` depends on
  `heiko-ui-remote` alone ([0030](decisions/0030-host-halves-are-crates.md)).
- `heiko`, the app, is `heiko-ui` with the Luau plugins' host halves
  (heiko-codemode's and heiko-luau-plugins'), which only it links: `heiko-ui`
  builds while Luau's C++ does.
- Plugins, under `crates/plugins/`, depend on `heiko-agent` (and
  `heiko-ai` for message types). Core crates never depend on a plugin
  ([0006](decisions/0006-plugin-crates.md)). Each plugin brings its own
  UI through `heiko-ui-plugin`, and has no headless form
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
    in flight. heiko sends no `stream_id`, which the plan route may not
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
- **MCP servers belong to the host, not to a run.** heiko-mcp keeps one
  plugin per repository on the host, whose connections every run in the
  repository shares; they close when the host goes
  ([mcp.md](reference/mcp.md)).
- **Store access:**
  - all writes go through one writer connection;
  - reads use a read-only pool;
  - each turn costs one write transaction.

## I/O boundary

The protocol logic in `heiko-ai` does no I/O. It is written as plain state
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

| pi (TypeScript)                                    | heiko-agent (Rust)                                                   |
| -------------------------------------------------- | ------------------------------------------------------------------ |
| `pi-ai` `AssistantMessageEvent` + shared `partial` | `heiko_ai::event::AssistantEvent` (owned deltas) + `Accumulator`     |
| `openai-responses-shared.ts`                       | `heiko_ai::responses`                                                |
| `openai-codex-responses.ts` WebSocket cache        | `heiko_ai::ws` (plan endpoint, one lane per connection)              |
| `pi-agent-core` `Agent` / `agentLoop`              | `heiko_agent::agent::Agent` / `Run`                                  |
| `AgentTool.execute(id, params, signal, onUpdate)`  | `AgentTool::call(args, ToolCtx { cancel, updates })`               |
| `beforeToolCall` / `afterToolCall` / listeners     | `PluginRun` (`before_tool` / `after_tool_result` / `on_event`)     |
| steering queue                                     | `Run::steer`                                                       |
| follow-up queue                                    | not ported (start another run)                                     |
| JSONL session tree                                 | `runs` + `messages` tables; forks via `parent_run_id` / `fork_seq` |
| compaction (`coding-agent/src/core/compaction`)    | `heiko-compaction` (a plugin)                                        |
| faux provider                                      | `heiko_testing::scripted::ScriptedModel`                             |
| built-in tools                                     | `heiko-tools-host`                                                   |
