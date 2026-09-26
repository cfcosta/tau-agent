# Plan

This plan covers the scope, size, milestones and risks for tau-agent. The
fixed decisions behind it are in [`decisions/`](decisions/). Sizes and
durations are estimates for one senior Rust engineer, and exclude tests.

## Scope

### Core crates (~7.5k lines)

| Crate / module                                                 | Est. lines | Difficulty (1–5) | Key dependencies                            |
| -------------------------------------------------------------- | ---------: | :--------------: | ------------------------------------------- |
| `tau-ai`: types, events, accumulator                           |       0.8k |        2         | `serde_json` (`preserve_order`), `indexmap` |
| `tau-ai`: Responses conversion + event processor               |       1.0k |        3         | `serde`, `jiter`                            |
| `tau-ai`: WebSocket pool, lanes, continuation                  |       0.9k |        4         | `tokio-tungstenite` (rustls)                |
| `tau-ai`: retry, cost, API-key auth, model table               |       0.4k |        2         | `tokio-util`                                |
| `tau-agent`: loop, `Agent`, `Run`, steering, cancel            |       1.3k |        3         | `tokio`, `futures`                          |
| `tau-agent`: tool trait, coercion + validation, strict schemas |       0.7k |        3         | `jsonschema`, `schemars`                    |
| `tau-agent`: typed results, sub-agents, limits, forks, hooks   |       0.9k |        3         | none                                        |
| `tau-agent`: compaction                                        |       0.5k |        2         | none                                        |
| `tau-store`: runs, messages, forks                             |       0.6k |        2         | `sqlx` (`sqlite`, `macros`, `migrate`)      |
| `tau-testing`: scripted model, generators, run replay          |       0.4k |        1         | `hegeltest`                                 |

### Optional crate (~2k lines)

| Crate                                                | Est. lines | Difficulty (1–5) | Key dependencies                                       |
| ---------------------------------------------------- | ---------: | :--------------: | ------------------------------------------------------ |
| `tau-tools`: read, bash, edit, write, grep, find, ls |       2.0k |        2         | `nix`, `similar`, `grep-searcher`, `ignore`, `globset` |

### Hardest part

The hardest part is the WebSocket layer. It has to:

- multiplex runs onto lanes over a shared socket;
- enforce the 16-in-flight limit;
- rotate connections before they expire;
- keep each lane's continuation chain correct through cancels,
  compactions and reconnects.

### Out of scope

- Any other LLM provider, any other OpenAI API, SSE, and OAuth.
- A CLI, TUI or RPC server.
- Settings files and `AGENTS.md` discovery.
- Skills and prompt templates.
- pi's branching session tree, `context_edit` and branch summaries.
- Follow-up queues.
- pi's TypeScript extension system. The Rust extension points are the
  `AgentTool` and `RunHook` traits.

## Milestones

### M1: OpenAI over WebSocket (weeks 1–2)

- [x] Message, content, usage and stop-reason types. The serde shape
      mirrors pi's JSON (camelCase, tagged by `role` / `type`).
- [x] `AssistantEvent`: owned deltas, 12 variants. Add an `Accumulator`
      that rebuilds the final `AssistantMessage`.
- [x] Conversion from transcript to Responses input items. It covers:
  - the `call_id|item_id` tool-call ids;
  - tool outputs;
  - reasoning items carrying `encrypted_content`.
- [x] Processing of `response.*` events: text, reasoning, function-call
      argument deltas, and the `completed`/`failed`/`incomplete`
      terminal events.
- [x] Incremental partial-JSON parsing of tool arguments. pi re-parses
      the whole buffer on every delta; we don't.
- [x] `response.create` bodies: WebSocket rules, clamped limits,
      encrypted reasoning for every reasoning model.
- [x] I/O-free core (see [`architecture.md`](architecture.md#io-boundary)):
  - [x] `ws::proto` lane and pool state machines, with no I/O and no
        clock;
  - [x] `ws::io` connection tasks and a driver that runs the pool;
  - [x] a `Connector` trait, with TLS (rustls, ring, webpki-roots) as
        the default.
- [x] Connection pool:
  - [x] one socket carries many lanes, one `stream_id` per run;
  - [x] at most 16 requests in flight per connection; a request past
        that waits, in order, on its lane's connection;
  - [x] connections rotate at 55 minutes;
  - [x] a 33rd lane, or a lane with no free in-flight slot, opens a new
        connection.
- [x] Per-lane continuation and the delta rule (see
      [`reference/openai-websocket.md`](reference/openai-websocket.md)).
- [x] Recovery ladder:
  - [x] `previous_response_not_found` → full resend;
  - [x] `websocket_connection_limit_reached` → reconnect, then full
        resend;
  - [x] any other error → an `error` event.
  - [x] recoveries are invisible to the caller; a cancelled response's
        tail is skipped.
- [ ] Idle timeout per connection.
- [ ] Optional warm-up with `generate: false`.
- [x] Retry classification on OpenAI error codes and HTTP status, not
      regexes over the error text.
- [x] Cost from usage: cached input pricing and service tiers. Backed by
      the vendored models.dev table plus pi's OpenAI corrections.
- [x] `PoolStats` counters (see
      [`reference/openai-websocket.md`](reference/openai-websocket.md)).
- [x] `OpenAi` client with one `Session` per run.
- [ ] Test infrastructure (see [`reference/testing.md`](reference/testing.md)):
  - [x] `hegeltest` wired in; `tau_testing::block_on` on a paused
        current-thread runtime;
  - [x] shared generators for messages, transcripts and `response.*`
        event streams in `tau_testing::generators`;
  - [ ] `tau_testing::FakeOpenAi`, a turmoil host that speaks the
        Responses WebSocket protocol, with server-side continuation,
        limits and fault injection. Done except the 16 in-flight and
        32-lane limits and the 60-minute close;
  - [ ] CI Check tier, nightly tier, and the nightly live job with a
        budget cap. The live job waits for the live tests.
- [ ] Tests:
  - [x] the `tau-ai` properties from the testing inventory;
  - [ ] replay recorded `response.*` streams (needs live recordings);
  - [ ] delta-rule model test, including its extended variant (the
        extended variants are not written yet);
  - [x] `ws::proto` properties over generated event orders;
  - [x] transport tests in turmoil against `FakeOpenAi`, with
        Hegel-drawn fault schedules, asserting on `PoolStats`;
  - [x] the `tau-ai` known cases;
  - [x] model and pricing table checks;
  - [ ] live cases for plain, reasoning and tool turns, plus the
        continuation probe;
  - [ ] measure the delta hit rate.

### M2: Agent loop and tools (weeks 2–3)

- [x] `Agent` builder and `Run` handle (see [`reference/api.md`](reference/api.md)).
- [x] The loop, with the ordering from [`reference/agent-loop.md`](reference/agent-loop.md).
- [x] `AgentTool` and `TypedTool` traits. Schemas come from `schemars`.
- [x] Argument coercion before validation, matching typebox
      `Value.Convert`:
  - [x] a string number becomes a number;
  - [x] `null` on an optional field is treated as absent.
- [x] Strict-schema rewriting for OpenAI, ported from pi's
      `constrained-sampling.ts`.
- [x] `RunHook`:
  - [x] `before_tool` can block the call or change its arguments;
  - [x] a hook that errors blocks the call;
  - [x] `after_tool`;
  - [x] `on_event`, awaited in order.
- [x] Steering: messages are injected after the current tool batch.
- [x] Cancellation through a `CancellationToken`. Every tool call gets a
      result, even when the batch is aborted mid-way.
- [x] `tau-testing::ScriptedModel`, ported from pi's `faux.ts`.
- [x] The `tau-agent` properties from the testing inventory, including
      the loop model test over generated scripts, and the loop known
      cases.
- [x] `trybuild` compile-fail cases for `Agent`, `TypedTool` and
      `RunHook`.
- [ ] Retries of retryable model errors inside the loop (agent-loop.md,
      "Retries"). Needs the error code carried on the failed response;
      the classification itself is done (M1).
- [ ] Argument schemas that use JSON Schema 2020-12 `prefixItems` (which
      `schemars` emits for tuples) are not coerced item by item; pi has
      the same gap.

### M3: SQLite store (weeks 3–4)

- [x] Migrations `0001_runs.sql` (see [`reference/storage.md`](reference/storage.md)).
- [x] Writer pool with one connection and `BEGIN IMMEDIATE`, plus a
      read-only reader pool. WAL mode and `synchronous = NORMAL`.
- [x] Atomic per-turn append: the turn's messages and the run totals
      commit together.
- [x] Fork transcript as one `WITH RECURSIVE` query.
- [x] Compaction records. Loading a run trims everything before the
      latest compaction record.
- [x] Offline metadata in `.sqlx/` is committed.
- [x] CI runs `cargo sqlx prepare --check`.
- [ ] Metric: time spent waiting for the writer connection.
- [ ] The `tau-store` properties from the testing inventory, including
      the store model test (done) and the combined loop-and-store cancel
      test (needs the M2 loop).

### M4: Workflow primitives (weeks 4–5)

- [ ] `run_typed::<T>`:
  - [ ] schema from `T`, rewritten to strict form;
  - [ ] sent as the Responses `text.format`;
  - [ ] the final message is parsed as `T`.
- [ ] `Agent::as_tool`:
  - [ ] each call starts a child run on its own lane, with
        `parent_run_id` set;
  - [ ] cancelling the parent cancels the child;
  - [ ] the child's usage counts toward the parent's limits.
- [ ] `Limits`: turns, tokens, USD and wall clock. Checked after every
      turn. When a limit is hit the run ends with `StopReason::Limit`.
- [ ] `Checkpoint` and `Agent::fork`.
- [ ] `workflow_id` grouping, and a cost query per workflow.
- [ ] Compaction: triggered by a threshold, and once on context
      overflow. The next turn starts a fresh WebSocket chain.
- [ ] Three example workflows that double as integration tests: a typed
      pipeline, a supervisor with sub-agents, and a fork fan-out.
- [ ] The remaining `tau-agent` properties: limits, forks, compaction,
      with the compaction known cases.
- [ ] Live cases for damaged transcripts, forks, compaction and a real
      context overflow.
- [ ] `cargo mutants` clean on the modules listed in the testing doc.

### M5: `tau-tools` (weeks 6–7, optional)

- [ ] The seven tools as specified in [`reference/tools.md`](reference/tools.md).
- [ ] Search runs natively, with no `rg` or `fd` subprocesses and no
      binaries downloaded at runtime.
- [ ] The `tau-tools` properties, example tests and known cases from
      the testing doc, and an example test for every error string in
      `tools.md`.

## Risks

| Risk                                                                                                            | Mitigation                                                                                                                                                                                                                                                             |
| --------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| WebSocket mode is recent, and networks that block WebSocket upgrades stop every agent.                          | Surface connection failures clearly. Document the network requirement.                                                                                                                                                                                                 |
| One vendor.                                                                                                     | Keep `tau-ai` behind an `Llm` trait, which `ScriptedModel` already implements.                                                                                                                                                                                         |
| Continuation correctness across lanes: a cancel, a compaction or a reconnect must reset exactly the right lane. | An I/O-free `ws::proto` tested with properties over generated event orders, and transport tests in turmoil against `FakeOpenAi`, which rebuilds the input server-side. `PoolStats` from M1. A nightly live probe asserts the delta hit rate against the real endpoint. |
| SQLite has a single writer.                                                                                     | Batch appends per turn. Measure writer wait time. It only matters at very wide fan-out.                                                                                                                                                                                |
| No upstream to track. tau-agent reuses pi's ideas as of `2b0a123`; it does not follow pi's releases.            | Pull fixes from pi by hand when they are relevant.                                                                                                                                                                                                                     |
