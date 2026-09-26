# pi audit

- Audited repository: <https://github.com/earendil-works/pi>
- Commit `2b0a123` (2026-09-26), all packages at v0.87.1
- Line counts are `wc -l` over each package's `src/`.

## What pi is

pi is a TypeScript monorepo by Mario Zechner, Armin Ronacher and others,
with about 200k lines of source. The installable product is
`@earendil-works/pi-coding-agent`, a terminal coding agent.

| Package                                  | src lines | On the shipped path?              | Notes                                                                          |
| ---------------------------------------- | --------: | --------------------------------- | ------------------------------------------------------------------------------ |
| `pi-coding-agent`                        |       76k | yes                               | TUI 20k, experimental server 11.2k, headless core ~30k, CLI 3.5k, RPC/print 2k |
| `pi-agent-core`                          |     33.5k | only `Agent` + `agentLoop` (2.6k) | also a durable `AgentHarness` (16k) and `pico3` (8k), both prototypes          |
| `pi-ai`                                  |     24.5k | yes                               | 10 wire APIs, 42 providers, 8 OAuth flows                                      |
| `pi-tui`                                 |       19k | yes (TUI only)                    |                                                                                |
| `chord`                                  |      8.6k | no                                | experimental service/replicated-state runtime                                  |
| `durable`                                |        9k | no                                | Pico5 durable store; nothing imports it yet                                    |
| `protocol`, `client`, `server`           |        4k | no                                | CBOR remote-session protocol v8; source-only behind `PI_EXPERIMENTAL=1`        |
| `telemetry`, `session-backends`, `evals` |      4.4k | no                                |                                                                                |

Upstream moves fast: 3,157 commits since 2026-03-27, about 15 a day in
September. Releases come every few days, and breaking changes are
routine. tau-agent does not track upstream (see [`../plan.md`](../plan.md)).

## What tau-agent takes

| From pi                                       | Path                                                                                                                                | Becomes                                                                                            |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Assistant event union                         | `packages/ai/src/types.ts:732`                                                                                                      | `tau-ai::AssistantEvent`, with owned deltas instead of the shared `partial`, plus an `Accumulator` |
| Responses input conversion + stream processor | `packages/ai/src/api/openai-responses-shared.ts`                                                                                    | `tau-ai::responses`                                                                                |
| WebSocket continuation / delta rule           | `packages/ai/src/api/openai-codex-responses.ts:1438` (`getCachedWebSocketInputDelta`), plus the cache TTL and max age around `:866` | `tau-ai::ws`                                                                                       |
| Strict JSON-schema rewriting                  | `packages/ai/src/api/constrained-sampling.ts`                                                                                       | typed results and strict tool schemas                                                              |
| Tool-argument coercion + validation           | `packages/ai/src/utils/validation.ts:317`                                                                                           | `tau-agent` validation                                                                             |
| Partial JSON repair                           | `packages/ai/src/utils/json-parse.ts`                                                                                               | incremental parser in `tau-ai`                                                                     |
| Agent loop ordering                           | `packages/agent/src/agent-loop.ts`                                                                                                  | `tau-agent` loop (see [`agent-loop.md`](agent-loop.md))                                            |
| Steering queue                                | `packages/agent/src/agent.ts:143`                                                                                                   | `Run::steer`                                                                                       |
| Compaction                                    | `packages/coding-agent/src/core/compaction/`                                                                                        | `tau-agent::compaction` (see [`compaction.md`](compaction.md))                                     |
| Faux provider                                 | `packages/ai/src/providers/faux.ts`                                                                                                 | `tau-testing::ScriptedModel`                                                                       |
| Built-in tools                                | `packages/coding-agent/src/core/tools/`                                                                                             | `tau-tools` (see [`tools.md`](tools.md))                                                           |

## What tau-agent leaves

- the TUI and the CLI;
- RPC, print and JSON modes;
- the extension system (jiti-loaded TypeScript, about 45 events) and the
  package manager;
- settings, trust, and `AGENTS.md` discovery;
- skills and prompt templates;
- the other 9 wire APIs, the other 41 providers, OAuth, and the model
  catalog generator;
- the JSONL session tree, `context_edit`, and branch summaries;
- follow-up queues and cache warming;
- the durable harness, pico3, chord, durable, protocol, client and
  server.

## Behaviours to keep exactly

- **Event order.** `start` comes first. `done` or `error` ends the
  stream. Errors after start are events, not exceptions.
- **Tool calls.**
  - Preparation is sequential and execution is parallel.
  - `ToolEnd` events come in completion order.
  - Result messages come in source order.
- **Listeners and hooks are awaited in order.** They act as
  backpressure.
- **Coercion runs before validation.**
- **Encrypted reasoning.** Request `reasoning.encrypted_content` and
  replay it on full resends.
- **Tool limits and error strings,** as listed in [`tools.md`](tools.md).

## Bugs found in pi, and how tau-agent avoids them

| Severity | Bug                                                                                                                                                                                                   | Location                                                           | tau-agent                                                |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ | -------------------------------------------------------- |
| high     | `grep` drops matches on lines containing U+2028/U+2029. Node's `readline` splits on them, and the JSON parse failure is swallowed. The splitting was reproduced in Node. `find` has the same pattern. | `coding-agent/src/core/tools/grep.ts:169`, `find.ts:217`           | Searches natively; no subprocess, no line reader         |
| medium   | `edit` checks uniqueness in normalized space even for exact matches.                                                                                                                                  | `coding-agent/src/core/tools/edit-diff.ts:328`                     | Checks uniqueness in the space where the match was found |
| medium   | `runAgentLoopContinue` copies the context shallowly, then pushes into the caller's array.                                                                                                             | `agent/src/agent-loop.ts:143`                                      | The loop owns its transcript; callers never share it     |
| medium   | An abort mid-batch leaves tool calls without results.                                                                                                                                                 | `agent/src/agent-loop.ts:572`                                      | Synthetic "cancelled" results for every pending call     |
| medium   | Errored or aborted assistant turns are dropped, but their tool results are kept (orphans).                                                                                                            | `ai/src/api/transform-messages.ts:201`                             | Turns are persisted atomically with their results        |
| medium   | Session files have no locking, loading can write, and the leaf is lost on reload.                                                                                                                     | `coding-agent/src/core/session-manager.ts:668, 1103`               | SQLite transactions; no files                            |
| medium   | Possible double "started" when two RPC prompts race. Not reproduced.                                                                                                                                  | `coding-agent/src/core/agent-session.ts:1654` vs `:1473`           | No RPC layer; `Agent::start` always creates a new run    |
| perf     | Streamed tool arguments are fully re-parsed on every delta, which is quadratic.                                                                                                                       | `ai/src/utils/json-parse.ts`, adapters                             | Incremental parsing                                      |
| low      | Retry classification uses loose regexes over error text; a bare `500` anywhere counts.                                                                                                                | `ai/src/utils/retry.ts:26`                                         | Classifies on error codes and HTTP status                |
| low      | `find` says "limit reached" when the count equals the limit.                                                                                                                                          | `coding-agent/src/core/tools/find.ts:145`                          | Only when more results existed                           |
| debt     | Four parallel compaction and session implementations, with drifting prompts.                                                                                                                          | `agent/src/harness/compaction`, `coding-agent/src/core/compaction` | One implementation, ported from the shipped one          |
