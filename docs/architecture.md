# Architecture

## Crates

```
tau-agent/
├── crates/
│   ├── tau-ai/       # types, events, Responses conversion, WebSocket pool, retry, cost
│   ├── tau-agent/    # Agent, Run, loop, AgentTool + RunHook, typed results, limits,
│   │                 # sub-agents, forks, compaction
│   ├── tau-store/    # SQLite via sqlx: runs, messages, fork transcripts, migrations
│   ├── tau-testing/  # ScriptedModel, recorded-stream replay
│   └── tau-tools/    # optional: read, bash, edit, write, grep, find, ls
└── docs/
```

Dependency direction:

- `tau-agent` depends on `tau-ai` and `tau-store`.
- `tau-testing` depends on `tau-ai`.
- `tau-tools` depends only on the tool trait from `tau-agent`.

## Data flow for one turn

```
Agent::start(input, &store)
  └─ Run task
       ├─ store: INSERT runs (status = running)
       ├─ lane = pool.acquire_lane()                 # stream_id on a shared socket
       └─ loop
            ├─ transcript = store.transcript(run)     # fork ancestors included
            ├─ body = responses::build(instructions, tools, transcript, text.format?)
            ├─ lane.create(body)                     # delta rule: suffix + previous_response_id
            │    └─ response.* events → AssistantEvent stream → RunEvent (hooks awaited)
            ├─ tool calls? prepare sequentially → execute in parallel → results in source order
            ├─ store.append_turn(messages, usage)    # one write transaction
            ├─ limits / cancel / steering checks
            └─ compaction threshold? summarize → compaction record → next turn is a full resend
       └─ store: UPDATE runs (status, result)
```

## Concurrency model

- **One `OpenAi` client per process.** It owns a pool of WebSocket
  connections.
  - Each run gets a lane: a `stream_id` on one of those connections.
    Requests on a lane are FIFO; different lanes run concurrently.
  - Each connection is capped at 32 lanes and 16 in-flight responses.
    Past either cap, the pool opens another connection.
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
| `openai-codex-responses.ts` WebSocket cache        | `tau-ai::ws` (API-key endpoint, lanes)                             |
| `pi-agent-core` `Agent` / `agentLoop`              | `tau-agent::Agent` / `Run`                                         |
| `AgentTool.execute(id, params, signal, onUpdate)`  | `AgentTool::call(args, ToolCtx { cancel, updates })`               |
| `beforeToolCall` / `afterToolCall` / listeners     | `RunHook`                                                          |
| steering queue                                     | `Run::steer`                                                       |
| follow-up queue                                    | not ported (start another run)                                     |
| JSONL session tree                                 | `runs` + `messages` tables; forks via `parent_run_id` / `fork_seq` |
| compaction (`coding-agent/src/core/compaction`)    | `tau-agent::compaction`                                            |
| faux provider                                      | `tau-testing::ScriptedModel`                                       |
| built-in tools                                     | `tau-tools`                                                        |
