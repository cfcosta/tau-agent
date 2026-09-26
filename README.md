# tau-agent

A Rust library for running LLM agents inside your own program. With it,
agent workflows are plain async Rust: you sequence runs, join them, fork
them from a checkpoint, and hand one agent to another as a tool.

tau-agent reuses the core of [pi](https://github.com/earendil-works/pi)
(earendil-works/pi, audited at commit `2b0a123`, v0.87.1): its event model,
agent loop, tool validation, compaction and WebSocket continuation logic.
It leaves behind pi's product layers: the TUI, the CLI, extensions,
settings, and the other provider integrations.

## Fixed decisions

| Area         | Decision                                                                                                                                      |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Shape        | A library, not an application. No CLI, no TUI, no RPC server.                                                                                 |
| LLM          | OpenAI only, API keys only, the Responses API over its WebSocket mode (`wss://api.openai.com/v1/responses`). WebSocket is the only transport. |
| Storage      | Embedded SQLite through stock `sqlx`. Every query uses sqlx's compile-time-checked macros.                                                    |
| Coding tools | Kept out of the core, in an optional `tau-tools` crate.                                                                                       |

Each decision is recorded in [`docs/decisions/`](docs/decisions/).

## Status

Planning. The workspace has empty crate skeletons and no implementation yet. See [`docs/plan.md`](docs/plan.md).

## Documentation

| Document                                                                   | What it covers                                                                            |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| [`docs/plan.md`](docs/plan.md)                                             | Scope, size estimates, milestones with task checklists, risks                             |
| [`docs/architecture.md`](docs/architecture.md)                             | Crates, data flow, concurrency model                                                      |
| [`docs/reference/api.md`](docs/reference/api.md)                           | Public Rust API: `Agent`, `Run`, tools, hooks, limits, typed results, forks               |
| [`docs/reference/agent-loop.md`](docs/reference/agent-loop.md)             | Loop semantics and ordering guarantees inherited from pi                                  |
| [`docs/reference/openai-websocket.md`](docs/reference/openai-websocket.md) | Responses WebSocket protocol, limits, continuation rule, recovery                         |
| [`docs/reference/storage.md`](docs/reference/storage.md)                   | SQLite schema, sqlx workflow, queries                                                     |
| [`docs/reference/compaction.md`](docs/reference/compaction.md)             | When and how long runs are summarized                                                     |
| [`docs/reference/tools.md`](docs/reference/tools.md)                       | Behaviour spec for the optional coding tools                                              |
| [`docs/reference/testing.md`](docs/reference/testing.md)                   | Testing strategy: property tests with Hegel, the property inventory, harness and CI tiers |
| [`docs/reference/pi-audit.md`](docs/reference/pi-audit.md)                 | Audit of pi: what it is, what we take, bugs we avoid                                      |
