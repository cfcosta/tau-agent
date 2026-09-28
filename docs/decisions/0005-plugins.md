# 0005: Plugins are crates behind per-run seams

- Status: proposed
- Date: 2026-09-28

## Context

We want to extend agents from separate crates: long-term memory,
automatic reasoning effort, rule checks on tool calls, and cheaper
compaction. Today the only extension points are tools and `RunHook`
(`before_tool`, `after_tool`, `on_event`), and compaction is built into
the loop.

pi extends its agent with runtime extensions that subscribe to events,
including a per-turn `context` transform. tau-agent cannot copy that
shape. On the Responses WebSocket, a request continues the previous
response only while the settings stay the same and the transcript only
grows, and a view re-applied every turn would break that chain.

## Decision

- A plugin is a Rust value implementing `Plugin`, added with
  `Agent::plugin`. There is no dynamic loading.
- Each run gets its own `PluginRun` per plugin, so plugin state is
  per-run by construction.
- Settings change only in `Plugin::start`, through a `RunPlan`, before
  the session opens.
- Context edits are stored rewrites, not per-turn views. Compaction
  becomes a plugin of that seam.
- Model calls a plugin makes are charged to the run.

The interfaces are in [`../reference/plugins.md`](../reference/plugins.md).

## Consequences

- Plugins can do everything the first four need without touching the
  delta rule. Settings are chosen once, and a rewrite costs exactly one
  full resend.
- `tau-store` gains `context` and `plugin` entry kinds. `compaction`
  rows keep loading.
- `RunHook` stays for simple cases, as an adapter.
- A plugin cannot change settings mid-run. A workflow that needs that
  starts a new run.
