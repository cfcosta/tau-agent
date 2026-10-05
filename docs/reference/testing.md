# Testing

This document sets the testing strategy for tau-agent and the quality bar
every test must meet. It covers:

- the rules that make a test worth keeping;
- property-based testing with Hegel, which is the default for new tests;
- the property inventory for each crate;
- the test harness: `ScriptedModel`, the fake OpenAI server, recorded
  streams and sqlx;
- the live tests against the real endpoint;
- CI tiers, mutation testing and the review checklist.

## Principles

1. **A test states a behaviour, not an implementation.** It names a rule
   from the reference docs and checks it. If the rule is not written
   down, write it down first.
2. **Prefer a property to an example.** An example test checks one
   input. A property checks the rule over a whole input space and
   shrinks a failure to a minimal case. Use an example test only when
   the table in [When to write an example test](#when-to-write-an-example-test)
   allows it.
3. **Every property has an oracle.** The oracle is a reference
   implementation, a model, a round trip, or a metamorphic relation.
   "It does not panic" is not an oracle.
4. **Inputs are valid by construction.** Generators build valid values
   directly. They do not produce random values and then reject most of
   them.
5. **Tests are deterministic.** No wall clock, no real network, no
   shared global state, no sleeps. Time is tokio's paused clock. Ids and
   timestamps are injected.
6. **A failure is a finding.** A shrunk counterexample becomes a named
   regression case before the fix lands. A surviving mutant is a
   missing assertion.
7. **Tests run offline.** Nothing in the default test run needs
   a ChatGPT sign-in, `DATABASE_URL` or network access.
8. **Test against reality at the edges.** A model we wrote can share
   our misunderstanding of the server. So the transport is tested
   over a simulated network against a fake server that keeps
   OpenAI's state rules (`FakeOpenAi` in turmoil), the tools against a
   real filesystem (a tempdir), and the request shapes against the real
   endpoint (the nightly live tier).
9. **Known bad inputs are kept.** Inputs that broke pi, or that broke
   us, stay in the suite as fixed cases, next to the property that
   should have caught them. See [Known cases](#known-cases).

## Tools

| Tool                           | Use                                                                  |
| ------------------------------ | -------------------------------------------------------------------- |
| `hegeltest` (lib name `hegel`) | Property-based tests: generators, shrinking, stateful model tests    |
| `cargo nextest`                | Test runner, locally and in CI                                       |
| `tau-testing`                  | `ScriptedModel`, `FakeOpenAi`, shared generators, replay, async glue |
| `tau_ui_plugin::testing`       | Feature `testing`: `FakeRun`, `run_ctx`, `fold`, `RepoValues`        |
| `tokio::time::pause`           | Deterministic time for retries, timeouts, idle timers and rotation   |
| `turmoil`                      | Deterministic simulated network and clock for the transport tests    |
| `tempfile`                     | Filesystem fixtures for `tau-tools`                                  |
| `cargo mutants`                | Test strength on the core modules                                    |
| `cargo sqlx prepare --check`   | The committed `.sqlx/` metadata matches the queries                  |
| `trybuild`                     | Compile-fail tests for the public API                                |
| `cargo bench`                  | Throughput benchmarks: `tau-ai` `transport`, `tau-tools` `grep`      |

All of these are in the dev shell, except the crates, which come
through Cargo. `hegeltest` is declared once in
`[workspace.dependencies]`. It is a dev-dependency of every crate that
has tests, and a normal dependency of `tau-testing`, which exports the
shared generators.

Hegel is used rather than `proptest` because it shrinks through
Hypothesis's engine. That engine keeps generated values valid while it
shrinks, so generators need no custom shrink logic. Hegel also has
stateful model testing built in.

## Benchmarks

Two benchmarks measure throughput. Each is a plain `main` (no harness)
that takes its sizes from the environment and prints its results:

- `cargo bench -p tau-ai --bench transport`: many concurrent sessions
  against an in-process server on in-memory streams, each growing its
  transcript turn by turn. It reports turns per second, turn latency
  percentiles, and the server's count of full and delta requests.
  `RUNS`, `TURNS`, `PAYLOAD`, `DELTAS` and `THINK_MS` set the load.
- `cargo bench -p tau-tools --bench grep`: the `grep` tool on a
  generated tree, sized by `FILES` and `FILE_KB`.

Neither runs in CI; run them before and after a change on the hot path.

## Layers

| Layer     | What it covers                                                                         | Harness                                                          |
| --------- | -------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| Property  | Pure logic: `ws::proto`, conversions, parsers, schema rewrites, truncation, cut points | Hegel; `tests/` for public APIs, `#[cfg(test)]` for private code |
| Model     | Stateful components: WebSocket pool and lanes, the store, the agent loop               | `#[hegel::state_machine]` against a simple in-memory model       |
| Transport | The `ws::io` driver: pool, lanes and recovery over a WebSocket                         | turmoil simulation with `FakeOpenAi`; Hegel draws the faults     |
| Replay    | `tau-ai` event processing against recorded `response.*` streams                        | fixtures under `crates/tau-ai/tests/fixtures/`                   |
| Scripted  | Workflow behaviour: typed results, sub-agents, forks, limits, plugins                  | `ScriptedModel` + `Store::memory()`, generated scripts           |
| Live      | Request shapes and continuation against the real endpoint                              | `--features live`; nightly, never in the Check tier              |
| API shape | Misuse of the public API does not compile                                              | `trybuild` compile-fail cases                                    |
| Mutation  | Strength of the tests on the delta rule, the loop and the store                        | `cargo mutants`                                                  |

## Choosing a property

Pick the first kind of oracle that fits:

1. **Differential.** Compare the code under test with a simpler
   implementation of the same thing: an incremental parser with
   `serde_json`, native grep with a naive line scan, a recursive CTE with
   a Rust walk over a `Vec`. This finds the most bugs per line of test.
2. **Model-based.** For anything with state, run random operation
   sequences against both the real component and a plain model (a
   `Vec`, a `BTreeMap`, a small struct), and compare them after every
   step.
3. **Round trip.** `decode(encode(x)) == x`, and
   `accumulate(render(x)) == x`.
4. **Metamorphic.** When there is no oracle, change the input in a way
   whose effect on the output is known. For example, re-chunking a
   stream must not change the result; converting LF to CRLF before an
   edit must give the CRLF form of the LF result.
5. **Algebraic laws.** Idempotence, identity and additivity, but only
   where the law is part of the spec. Do not invent algebra.

A property that only restates the implementation is a tautology. Delete
it.

A differential oracle must not share the dependency it is checking. If
`find` uses `ignore::WalkBuilder`, the reference walk must not. Use
`std::fs::read_dir` and a hand-written `.gitignore` subset, and keep the
tricky `.gitignore` layouts as [known cases](#known-cases).

## Writing Hegel tests

### Shape

```rust
use hegel::{TestCase, generators as gs};

/// Delta rule: the server's view after a delta equals a full resend.
#[hegel::test(test_cases = 500)]
fn delta_reconstructs_full_input(tc: TestCase) {
    let turns = tc.draw(tau_testing::generators::lane_history());
    // ...
}
```

- `#[hegel::test]` adds `#[test]` itself. Do not add both.
- Start each property with a doc comment that names the rule and the
  reference doc it comes from.
- Name tests after the rule, not after the function:
  `cut_point_never_splits_tool_call`, not `test_find_cut_point`.
- Use `tc.note(...)` for derived values that help explain a failure.

### Case counts

| Kind of test                           | `test_cases`      |
| -------------------------------------- | ----------------- |
| Cheap pure function                    | 500               |
| Default                                | 100 (Hegel's own) |
| Model test, or one that touches SQLite | 50–100            |
| Filesystem (`tau-tools`)               | 50                |
| Nightly variant                        | from `hegel.toml` |

A nightly variant is a second test with the same body,
`#[ignore = "nightly"]`, and `#[hegel::test(profile = "nightly")]` (pure
properties, 10,000 cases) or `profile = "nightly_slow"` (a simulated
server, a store or an agent loop per case, 1,000 cases). The profiles
live in the workspace's `hegel.toml`, and draw fresh seeds even on CI,
so each nightly run explores new cases. The nightly job runs them with
`cargo nextest run --run-ignored only`. Put the body in a plain function
so both tests share it.

A count written in the test (`test_cases = 500`) beats a profile and
`HEGEL_TEST_CASES`. Write one only where the test needs a count of its
own.

### Generators

- **Shared generators live in `tau_testing::generators`.** Messages,
  transcripts, tool calls, JSON schemas, `response.*` event streams and
  lane histories are needed by several crates. Write each one once, as a
  `#[hegel::composite]` function.
- **Mind the dependency cycle.** `tau-testing` depends on `tau-ai`. A
  `#[cfg(test)]` module inside `tau-ai` that uses `tau-testing` would see
  two copies of `tau-ai`'s types, and the code would not compile. So
  properties in `tau-ai` (and in `tau-agent`, for the same reason) that
  use shared generators go in the crate's `tests/` directory, and they
  test the public API. Properties of private helpers stay in
  `#[cfg(test)]` modules with local generators.
- **Build valid values directly.** A transcript generator emits a tool
  result only after its tool call. A schema generator emits only schemas
  that the strict rewrite accepts. Use `tc.assume` only with a comment
  saying why the rejection rate is low. Never suppress
  `HealthCheck::FilterTooMuch`; fix the generator.
- **Shrink toward readable cases.** Small sizes, ASCII before Unicode,
  few turns before many. Keep bounds tight with `max_size` and
  `max_value`. A counterexample of 3 messages is useful; one of 300 is
  not.
- **Aim at the edges on purpose.** Generators for text must be able to
  produce the characters the code handles specially: CRLF, a BOM,
  U+2028/U+2029, NNBSP, smart quotes, combining marks, and multi-byte
  characters at chunk and truncation boundaries. Use `one_of!` to mix a
  small alphabet of these into ordinary text.
- **Derive when the type is plain.** Use `#[derive(DefaultGenerator)]`
  for configuration structs and small enums. Write a composite for
  anything with invariants.

### Async code

Hegel test bodies are synchronous. Drive async code on a current-thread
runtime with paused time:

```rust
tau_testing::block_on(async {
    // tokio::time::sleep advances instantly; no real waiting.
});
```

`tau_testing::block_on` builds
`tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true)`.
A paused current-thread runtime makes the order of tasks and timers
repeatable, so a failure replays exactly.

Do not spawn threads that call `tc.draw`. Draw everything first, then run
the async part.

**Never combine paused time with real sockets.** When time is paused and
the runtime has nothing to do, tokio moves the clock to the next timer.
Waiting on a real socket counts as having nothing to do. A test that
waits on a loopback socket under paused time can therefore fire idle
timeouts and connection rotation at random. Network tests run in
turmoil, whose network and clock are both simulated.

The same holds for SQLite through sqlx: its driver waits on worker
threads, which paused time treats as idle, so sqlx's own timeouts could
fire. Store tests run on a normal current-thread runtime instead. They
stay deterministic because they never read the clock and every
timestamp is written by SQLite and never asserted on.

Loop tests do run a store under paused time, and `Store::memory()`
sets no timeouts so nothing fires spuriously. The clock still jumps
while a write waits: the pool's acquire deadline is a pending timer, so
an idle runtime moves time to it. A run's elapsed time is therefore
meaningless under paused time, and a test with a `timeout` limit runs on
the real clock (its scripted turns have no delays, so it stays fast).

### Testing the WebSocket layer

The WebSocket layer is split at its I/O boundary (see
[`architecture.md`](../architecture.md#io-boundary)), and each side is
tested on its own terms:

- **`ws::proto`, with properties.** Hegel generates sequences of
  events for the pool and lane state machines: requests from several
  runs, each on a connection of its own, cancels, closes, errors and timer
  expiries with their timestamps. The properties compare the actions
  against a model. Because the state machine does no I/O, every
  ordering of concurrent events is just a different generated sequence,
  and a failure shrinks to the shortest one.
- **`ws::io`, in turmoil.** The real driver runs in a turmoil
  simulation, connected through a `Connector` to `FakeOpenAi` on a
  simulated host. These tests check what the state machine cannot: that
  frames are really written and read, that the driver carries out every
  action, and that timers, reconnects and cancels work with a real
  WebSocket codec. Keep them few and focused; the rules belong in the
  `ws::proto` properties.

shuttle and loom are not used. With one task per connection and no locks
shared between lanes, the orderings they would explore are covered by
the generated event sequences. Revisit this if shared-state concurrency
appears outside a single task: then add `shuttle-tokio` tests for that
code.

### Stateful model tests

Use `#[hegel::state_machine]` for the pool, the lanes, the store and the
loop:

- Each `#[rule]` performs one operation on both the real component and
  the model, and compares the results.
- Each `#[invariant]` checks a rule that must hold after every step.
- Guard a rule with `tc.assume` only for preconditions such as "the pool
  has a free lane". Such rules must still apply in most states.
- Use `hegel::stateful::Pool` for handles created earlier in the run,
  such as run ids, lanes and checkpoints, and draw from it with
  `values_reusable()` or `values_consumed()` so the choice shrinks.
- Run a machine with `hegel::stateful::machine(m).steps(n).run(tc)`;
  raise `n` only in nightly variants.
- `#[invariant(always_run)]` checks every step; a plain `#[invariant]`
  is sampled between steps.

### Failures and regressions

- The local example database is `.hegel/` in each crate's directory,
  because tests run from there. It is gitignored. Hegel replays saved failures first on the next run.
- CI disables the database and derandomizes seeds. Hegel does this by
  itself when it detects CI.
- When a property fails, keep the shrunk input as a regression before
  fixing the bug:
  - `#[hegel::explicit_test_case(name = value)]` on the property, when
    the input is short enough to write out; or
  - a named example test with a comment linking the property.
- `#[hegel::reproduce_failure("…")]` is for local debugging only. The
  blob depends on the Hegel version, so never commit it.

## When to write an example test

Example tests are allowed for:

| Case                                      | Why                                                                                                                             |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Exact strings from [`tools.md`](tools.md) | Models are tuned to them; the exact text is the spec                                                                            |
| Wire shapes pinned by pi or OpenAI        | A golden JSON fixture is the clearest statement of the shape                                                                    |
| Recorded streams                          | They are real server output, which a generator only approximates                                                                |
| Regressions from a shrunk counterexample  | They keep a found bug found                                                                                                     |
| The API examples in [`api.md`](api.md)    | They are M4's acceptance tests and documentation                                                                                |
| Table-driven classification               | Retry codes and HTTP statuses: the table is finite; test all of it                                                              |
| Known cases                               | Inputs that broke pi or us; see [Known cases](#known-cases)                                                                     |
| Compile-fail cases                        | `trybuild`: the API rejects misuse at compile time                                                                              |
| Model and pricing table                   | Every model has a context window, an output limit and prices, and the prices match the values pinned from OpenAI's pricing page |

Everything else starts as a property.

## Property inventory

Each crate's properties are listed below, with the oracle for each.
Implement a property in the same milestone as the code it covers. The
list is a floor, not a ceiling.

### `tau-ai`

| Property                                                                                                                                                                                                                                                                                                                                                              | Oracle       |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| Message types: `from_json(to_json(m)) == m`, and the JSON uses pi's shape (camelCase, tagged by `role` / `type`)                                                                                                                                                                                                                                                      | Round trip   |
| `Accumulator` over any `AssistantEvent` stream built from a message gives back that message                                                                                                                                                                                                                                                                           | Round trip   |
| Re-chunking the deltas of a stream at arbitrary points does not change the accumulated message                                                                                                                                                                                                                                                                        | Metamorphic  |
| Event processor: render a generated response as `response.*` events, with arbitrary delta splits; processing gives back the response                                                                                                                                                                                                                                  | Round trip   |
| Every processed stream starts with `start` and ends with exactly one of `done` or `error`                                                                                                                                                                                                                                                                             | Invariant    |
| Incremental JSON parser, fed the arguments in any chunking, agrees with `serde_json` on the complete text                                                                                                                                                                                                                                                             | Differential |
| Each partial parse is consistent with the final value: no field seen early is later changed or dropped                                                                                                                                                                                                                                                                | Invariant    |
| Transcript → Responses input: tool-call ids split into `call_id` and `item_id`, and every tool output follows its call                                                                                                                                                                                                                                                | Invariant    |
| **Delta rule:** for any lane history, `baseline + sent_input` equals the full input the request would otherwise carry                                                                                                                                                                                                                                                 | Differential |
| Delta rule: a change to anything other than `input`, or to the baseline prefix, forces a full resend with no `previous_response_id`                                                                                                                                                                                                                                   | Metamorphic  |
| Cancel, compaction, reconnect and `previous_response_not_found` each make the lane's next request a full resend                                                                                                                                                                                                                                                       | Model        |
| Pool (state machine): never more than one lane or one in-flight response per connection; requests on a lane stay FIFO; no new work goes to a connection older than 55 minutes; each lane goes where a reference selector says (own path, parent's, wait, free, new) and what a connection serves moves with it; at most `max_idle` free; no wait past `affinity_wait` | Model        |
| Backoff delay for attempt `n` lies in `[0, base × 2ⁿ]`, and the number of attempts never exceeds the limit                                                                                                                                                                                                                                                            | Invariant    |
| Every strict prefix of a valid event stream ends in `error`, never in `done`                                                                                                                                                                                                                                                                                          | Metamorphic  |
| Unknown server event types anywhere in a stream are ignored and do not change the result                                                                                                                                                                                                                                                                              | Metamorphic  |
| Converted input never holds a `function_call` without its output, or a reasoning item without the output item it belongs to, including after an aborted or errored turn                                                                                                                                                                                               | Invariant    |
| **Transport (turmoil, against `FakeOpenAi`):** the input the server rebuilds from its cache plus each delta equals the full input of that turn                                                                                                                                                                                                                        | Differential |
| Transport: after any recovery, the caller sees exactly one `start` and no `error` for the turn, unless recovery gives up                                                                                                                                                                                                                                              | Model        |
| Transport: `PoolStats` matches the model: `delta_requests` equals requests minus the forced full resends, and a new lane reuses a connection a closed lane left                                                                                                                                                                                                       | Model        |
| Transport: `websocket_connection_limit_reached` reconnects once, and only before any output was emitted; after output it becomes an `error` event                                                                                                                                                                                                                     | Model        |
| A turn cancelled before its terminal event records zero usage                                                                                                                                                                                                                                                                                                         | Model        |
| Cost is additive: `cost(a + b) == cost(a) + cost(b)` for usages of the same model and tier                                                                                                                                                                                                                                                                            | Algebraic    |

The delta rule is the riskiest code in the project. Its model test must
also run in the nightly tier and under `cargo mutants`.

### `tau-agent`

| Property                                                                                                                                                               | Oracle       |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| Strict schema rewrite: every object in the output has all properties required and `additionalProperties: false`                                                        | Invariant    |
| Strict schema rewrite is idempotent                                                                                                                                    | Algebraic    |
| A value valid under the original schema, with missing optional fields set to `null`, is valid under the strict schema                                                  | Metamorphic  |
| A schema with a string `format` has a strict form exactly when OpenAI's strict mode takes the format, and a strict form holds no other                                 | Invariant    |
| Coercion leaves a value that already validates unchanged                                                                                                               | Invariant    |
| Coercion is idempotent                                                                                                                                                 | Algebraic    |
| A number, a boolean or a one-element array gives the same result as its string or scalar form after coercion                                                           | Metamorphic  |
| `before_tool` plugins: the result equals a left fold over the plugins in which the first `Block` or error wins                                                         | Model        |
| **Loop, over generated scripts** (tool calls, tool results that succeed, fail or take time, steering, cancel at any point):                                            | Model        |
| · every tool call in the transcript has exactly one result, including after a cancel                                                                                   |              |
| · result messages are in source order; `ToolStart` is in source order; `ToolEnd` is in completion order                                                                |              |
| · the event stream matches the grammar `RunStart (TurnStart … TurnEnd)* RunEnd`, and nothing follows `RunEnd`                                                          |              |
| · steered messages appear after the tool batch that was running, in the order they were sent                                                                           |              |
| · the persisted usage equals the sum of the turns' usage                                                                                                               |              |
| · in a parallel batch, tools whose virtual-time intervals could overlap do overlap; with any `Sequential` tool, no two intervals overlap                               |              |
| · grouped calls start together, and no two groups overlap; groups start in the order their first calls come                                                            |              |
| · a tool call cut off by a `length` stop never runs                                                                                                                    |              |
| · a `ToolUpdate` sent after the tool's future resolved produces no event and no panic                                                                                  |              |
| Steering sent from inside `on_event` or `before_tool` is drained at the next drain point, exactly once                                                                 | Model        |
| A slow `on_event` subscriber holds its run: the run does not finish until the subscriber returns, and other runs keep going                                            | Model        |
| Arguments changed by `before_tool` are validated again; invalid ones yield an error result and the tool never runs                                                     | Model        |
| Limits: a run ends with `StopReason::Limit` if and only if a limit was exceeded after some turn, and child usage counts                                                | Model        |
| A fork's transcript equals the parent's transcript up to the checkpoint, followed by the fork's own messages                                                           | Model        |
| (`tau-compaction`) Compaction cut point never falls between a tool call and its result, and never on a tool result                                                     | Invariant    |
| The kept suffix holds at least `keep_recent_tokens`, unless the whole transcript holds fewer or snapping past an oversized tool result moved the cut                   | Invariant    |
| The token estimate never decreases when a message is appended                                                                                                          | Invariant    |
| With no reported usage anywhere, the estimate is `chars / 4` over every message                                                                                        | Differential |
| A summary that stops with `length` or `error`, or that calls a tool, fails compaction and writes nothing                                                               | Model        |
| Repeated compactions: a second compaction runs only when the kept messages no longer fit, and summarizes messages the first one kept once they leave the recent window | Model        |

### `tau-store-sqlite`

| Property                                                                                                                                                    | Oracle       |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| State machine over create run, append turn (context rewrites and plugin records included), fork and finish, against a `Vec`-based model; every read matches | Model        |
| After any sequence of appends, a run's `seq` values are `0..n` with no gaps                                                                                 | Invariant    |
| The recursive-CTE transcript equals a Rust walk up the fork chain over the model                                                                            | Differential |
| Loading a run drops everything before its latest context entry, and leaves plugin records out                                                               | Model        |
| A failed append leaves neither messages nor usage totals behind                                                                                             | Model        |
| Loop and store together: a cancel or a write failure between tool completion and persistence leaves no partial turn                                         | Model        |
| Message bodies round-trip through the `body` column unchanged                                                                                               | Round trip   |

Store tests use `Store::memory()`. Each Hegel case opens a new store, so
cases do not share state.

### `tau-tools`

| Property                                                                                                                                                                               | Oracle       |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| `truncate_head` output is a prefix of whole lines, within `MAX_LINES` and `MAX_BYTES`                                                                                                  | Invariant    |
| `truncate_tail` output is a suffix (after `…` for a cut line), within the limits, and valid UTF-8 at every cut                                                                         | Invariant    |
| Truncation is idempotent, and input already within the limits is returned unchanged                                                                                                    | Algebraic    |
| `edit` with exact, unique, non-overlapping edits equals a naive reference that applies them to the string                                                                              | Differential |
| `edit` on a CRLF file, or a file with a BOM, equals the LF edit with the line ending and BOM restored                                                                                  | Metamorphic  |
| In fuzzy mode, lines that no edit touches keep their original bytes                                                                                                                    | Invariant    |
| `edit` rejects overlapping edits and edits that match more than once, in the space where they matched                                                                                  | Invariant    |
| `grep` over a generated tree equals a naive regex scan of each file's lines, including lines with U+2028/U+2029                                                                        | Differential |
| `find` equals `globset` matching over a naive walk; "limit reached" appears only when more results existed                                                                             | Differential |
| `ls` output is sorted case-insensitively, with a trailing `/` on directories and dotfiles included                                                                                     | Invariant    |
| `read` with `offset` and `limit` equals the same slice of the file's lines                                                                                                             | Differential |
| Applying the diff `edit` returns to the original content gives exactly the bytes written, in exact and fuzzy mode                                                                      | Round trip   |
| A failed `edit` leaves the file byte-for-byte unchanged                                                                                                                                | Invariant    |
| Per-path lock (state machine, paused time): overlapping `edit`/`write` calls on one file, or on a file and a symlink to it, never interleave their read-modify-write                   | Model        |
| Path resolution: for a generated name, a file created under a macOS variant (NNBSP before AM/PM in either case, NFD, curly apostrophe) is found from the typed form                    | Differential |
| Images: for a generated size, a PNG built in the test comes back within 2000×2000 and 4.5 MB, with its aspect ratio kept; the format is detected by magic bytes whatever the extension | Invariant    |
| `bash` output re-chunked at any byte boundary, including inside a multi-byte character, gives the same result                                                                          | Metamorphic  |
| `bash` keeps `truncate_tail` of the full output, and the spill file holds the full output, whether the limit hit was lines or bytes                                                    | Differential |
| `bash` progress updates stay under a fixed bound however many chunks arrive                                                                                                            | Invariant    |
| Terminal mode: plain output (no escapes, no `\r`), in reads of any size, gives the model the text, the "was it cut" flag, the totals and the spill that pipes give                     | Differential |

`bash` also has example tests:

- the 100 ms idle window, at 99 ms and 100 ms, under paused time; each
  new chunk restarts the window;
- `kill -KILL $$` and `kill -TERM $$` report exit codes 137 and 143, and
  keep the output printed before the kill;
- a grandchild process is killed with its group on cancel and timeout.

In terminal mode (the `terminal` feature), example tests cover the byte
stream (every byte once, in `seq` order, before the tool returns), the
result's details on success, failure, timeout and cancel, and their
round trip through the store.

### `tau-terminal`

| Property                                                                                                                 | Oracle     |
| ------------------------------------------------------------------------------------------------------------------------ | ---------- |
| Plain output with `\r\n` line ends, chunked anywhere, reads back from `Terminal::text` as itself with `\n` line ends     | Round trip |
| With recording on, all the recorded text then `text` is the whole plain output, whatever the chunking and the scrollback | Round trip |

Example tests cover `\r` redraws, SGR colors and wide characters in the
snapshot, the scrollback cap, VT replay, the alternate screen and
resizing. The PTY runner's tests run real processes: stdout and stderr
are a terminal and stdin is not, the size and environment, the
controlling terminal, prompts that read stdin or `/dev/tty` end at
once, every byte arrives in order, the exit status, killing the process
group, and late output from a grandchild.

Every string in the error table of [`tools.md`](tools.md#error-strings)
has an example test. Tests that rely on `chmod` skip themselves when
running as root, because root ignores file modes.

## ScriptedModel

`ScriptedModel` is ported from pi's faux provider
(`packages/ai/src/providers/faux.ts`). It implements `tau_ai::Llm`, and
replays scripted turns:

```rust
let llm = ScriptedModel::new()
    .turn(|t| t.thinking("…").tool_call("search", json!({"q": "tokio"})))
    .turn(|t| t.text("Found it.").usage(1200, 80));
```

- **Assertions.** It records every request it receives, so tests can
  assert on transcripts and on which tools were offered.
- **Script exhaustion.** Running out of turns fails the test with a
  clear message. It never hangs.
- **Errors.** It can emit error events to exercise the retry and
  overflow paths:
  - `rate_limit`;
  - `context_length_exceeded`;
  - a dropped connection.
- **Generated scripts.** `tau_testing::generators::script()` draws a
  whole script: turn count, tool calls per turn, tool outcomes and
  delays, steering points and a cancel point. The loop's model tests are
  built on it.

- **Cache simulation.** Like pi's faux provider, it reports
  `cached_tokens` for the part of a request that repeats an earlier
  request's prefix. Cost and compaction code then sees realistic cache
  numbers.
- **Early failure.** Besides error events, it can fail before the stream
  starts, as a failed connection does.

## Fake OpenAI server

`tau_testing::FakeOpenAi` is a turmoil host that speaks the Responses
WebSocket protocol, over plain `ws://` on the simulated network. For
tests that run tau on a real runtime, such as the host's, `listen()`
serves it on a local TCP port from a thread of its own, and
`LocalConnector` reaches it.
`ScriptedModel` replaces the whole `Llm`; `FakeOpenAi` replaces only the
far end of the socket, so the real pool, lanes, delta rule and recovery
ladder run. TLS is not simulated; the live tier covers it.

- **Recording.** It numbers connections and records every frame it
  receives, with the connection number.
- **Scripted replies.** Each request is answered from a script of
  `response.*` events, or from a generated response.
- **Server-side continuation.** It keeps the responses of each
  connection in memory, as OpenAI does. A request with a
  `previous_response_id` the connection does not hold gets
  `previous_response_not_found`. The cache is dropped after an error and
  when the connection closes.
- **Prompt cache per connection.** As on OpenAI's route, a request
  reads from cache the longest prefix it shares with a prompt that
  connection saw (head of model, tools and effort, then instructions up
  to their first difference, then input items; four bytes a token), and
  nothing another connection saw. Each `Received` records
  `input_tokens` and `cached_tokens`; `report_cache()` puts them in the
  completed response's usage. Connection-affinity tests assert on it.
- **Oracle for the delta rule.** For each request it rebuilds the full
  input from its cache plus the delta. Tests compare that with the full
  input the turn would have sent without continuation.
- **Limits.** It enforces 16 in-flight responses per connection (a
  request past that is refused and recorded as a violation, which a
  correct client never causes). tau sends no `stream_id`, so the fake
  tracks no named lanes. On turmoil's clock it closes a connection at
  60 minutes. `Reply::Delay` holds a reply back while the connection
  keeps reading, so responses on different connections overlap.
- **Upgrades.** It records each upgrade's `Authorization` header and
  can refuse an upgrade with a status and body. Plan tests pair it
  with `FakeChatGpt`, which signs in and hands out the bearer token.
- **Faults.** It can drop the connection before the first event or in
  the middle of a stream, delay events, and insert event types the
  client does not know. turmoil adds network faults: holding and
  releasing messages, and partitioning and repairing hosts.
- **Fault schedules come from Hegel.** A test draws a fault plan (which
  fault, and at which simulation step) and applies it between calls to
  `Sim::step`. turmoil's own random faults and latency are turned off,
  and its seed is drawn by Hegel. So a failing run shrinks to the
  smallest fault plan that still fails, and replays exactly.

There is one `FakeOpenAi`, configured per test. Do not write ad-hoc fake
sockets in individual tests.

## Live tests

Live tests run against `wss://api.openai.com/v1/responses` with the
`live` feature and a saved ChatGPT sign-in with plan usage. They check
what no fake can: that the server accepts our requests and honours our
continuation.

- **Credentials** come only from the saved sign-in in
  `$XDG_CONFIG_HOME/tau/chatgpt/` (`tau_ai::chatgpt::Store`), never
  from an API key ([0012](../decisions/0012-chatgpt-sign-in-only.md)).
  Without one, live tests fail with a clear message. They print no
  token.
- **Model and budget.** Use the cheapest model that supports the
  feature under test, and cap each test with `Limits::max_usd`.
- **No retries.** A live test is not retried. A failure prints the
  recorded frames so it can become a fixture.
- **Real assertions.** Each test asserts on responses and `PoolStats`.
  Logging a result is not an assertion.

Required cases:

| Case                                                               | Asserts                                                                                            |
| ------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------- |
| Plain text, reasoning, parallel tool calls, typed result           | the stream completes and parses                                                                    |
| Resend after an aborted turn that holds only reasoning             | the server accepts the converted input (no 400)                                                    |
| Resend after tool calls cancelled mid-batch                        | the server accepts the synthetic "cancelled" results                                               |
| Continuation probe: a 20-turn tool loop with padding in every turn | every turn after the first is a delta; the cached share of input tokens stays above a pinned floor |
| Fork and compaction                                                | the first turn after each is a full resend, and later turns are deltas again                       |
| A real context overflow                                            | it is classified as `context_length_exceeded`, and compaction recovers                             |

The probe is the only check that OpenAI still honours continuation as
documented. If its floor fails, re-read the WebSocket guide before
changing code.

## Known cases

A known case is a fixed input that broke pi or broke us. It lives next
to the property that covers its rule, as
`#[hegel::explicit_test_case(...)]` when the input fits on a few lines,
or as a named example test otherwise. Each one has a comment with its
source: a pi issue number, a pi test file, or our own regression.

Seed the suite with these cases from pi at `2b0a123`:

| Area       | Case                                                                                                               | pi source                                                                           |
| ---------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------- |
| Transport  | `previous_response_not_found` after a tool turn: full resend on a new connection, one `start`, no `error`          | `openai-codex-stream.test.ts:2216`                                                  |
| Transport  | an unknown event arriving before an error                                                                          | `openai-codex-stream.test.ts:2216`                                                  |
| Transport  | idle timeout before the first event vs after the stream started                                                    | `openai-codex-stream.test.ts:1753`, `:1855`                                         |
| Events     | a stream that ends with no terminal event; `incomplete` turning a provisional stop into `length`; `content_filter` | `openai-responses-terminal-event.test.ts`                                           |
| Events     | the internal partial-JSON buffer never reaches a persisted or emitted tool call                                    | `openai-responses-partial-json-cleanup.test.ts`                                     |
| Conversion | a turn aborted with only reasoning; tool calls with no result                                                      | `openai-responses-reasoning-replay-e2e.test.ts`, `tool-call-without-result.test.ts` |
| Loop       | a length-truncated tool call is never run                                                                          | `agent-loop.test.ts:408`                                                            |
| Compaction | a cut before an oversized trailing tool result                                                                     | #9740                                                                               |
| Compaction | no reported usage anywhere                                                                                         | #8328                                                                               |
| Compaction | the summary stream drops and is retried; cancel during the summary                                                 | #6647, #9340                                                                        |
| Compaction | a length-stopped summary is rejected                                                                               | #7048                                                                               |
| `bash`     | output still arriving at the end of the idle window; output after the result                                       | #5303, #5208                                                                        |
| `find`     | a nested `.gitignore` applies only to its subtree; a glob with `/` matches the full path; search from `/`          | #3303, #3302, #6104                                                                 |
| Paths      | lowercase `am`/`pm`; `~draft.md` and `@~draft.md` stay literal                                                     | `path-utils.test.ts:20`, `:159`                                                     |
| Images     | a JPEG with EXIF orientation after an XMP segment; a 1×1 BMP; magic bytes against a wrong extension                | `image-processing.test.ts:75`, `tools.test.ts:49`, `:200`                           |

## Recorded streams

A replay test reads one fixture file of raw WebSocket messages, one JSON
object per line, and feeds them through the processor. Each fixture
comes from a live run.

Required fixtures:

- plain text;
- reasoning with encrypted content;
- parallel function calls;
- a `previous_response_not_found` error;
- `response.incomplete` because the output-token limit was hit;

The recorded fixtures also calibrate the stream generator. Every event
type and field that appears in a fixture must be something the
generator can produce.

## Replaying stored runs

`tau-testing::replay(store, run_id)` turns a stored run into a
`ScriptedModel` script. A real run can then become a regression test for
the workflow code around it.

## sqlx in tests

- `Store::memory()` runs the migrations on `sqlite::memory:`.
- The query macros check against the committed `.sqlx/` metadata, so
  tests need no `DATABASE_URL`.
- CI runs `cargo sqlx prepare --check`.

## Mutation testing

`cargo mutants` runs on the modules where a silent bug costs the most:

- `tau-ai`: the delta rule, the lane state and the event processor;
- `tau-agent`: the loop, plugins, runs (typed results, forks,
  sub-agents), coercion and the strict schema rewrite;
- `tau-compaction`: the compaction rules and the plugin;
- `tau-fast-compaction`: the state, the decisions, the ledger and the
  plugin; `tau-jev`: answer checking;
- `tau-store-sqlite`: the append and transcript queries;
- `tau-tools`: `edit`, which rewrites files, and truncation.

A surviving mutant means a behaviour no test checks. Either add the
missing assertion, or record why the mutant is equivalent in
`.cargo/mutants.toml` with a comment.

## CI tiers

| Tier    | When       | Runs                                                                                                                             |
| ------- | ---------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Check   | every push | `nix fmt` check, `cargo clippy --all-targets -D warnings`, `cargo nextest run`, `cargo sqlx prepare --check`, `cargo deny check` |
| Nightly | once a day | everything in Check, plus `cargo nextest run --run-ignored only` (nightly properties) and `cargo mutants` on the modules above   |
| Live    | nightly    | the [live tests](#live-tests) with a budget cap; also run by hand to record new fixtures when the protocol changes               |

The Check tier must stay under five minutes. If it grows past that,
move cases to nightly variants rather than lowering the default counts.

## Anti-patterns

- A property that recomputes the result the same way the code does.
- A test whose only assertion is that nothing panicked.
- Generating random input and discarding most of it with `tc.assume`.
- Real sleeps, real clocks, `thread_rng`, or `HashMap` iteration order
  in an assertion.
- Asserting on `Debug` output or on error text, except for the pinned
  strings in [`tools.md`](tools.md).
- Mocking the component under test. Replace only the edges: the LLM
  (`ScriptedModel`), the network and the far end of the socket
  (turmoil and `FakeOpenAi`), the clock (paused tokio time or turmoil's
  clock), and the filesystem (a tempdir).
- Real sockets under paused time. See [Async code](#async-code).
- Testing a `ws::proto` rule through turmoil when a property over
  generated events would check it more directly.
- Retrying a failing test until it passes. A flaky test is a bug in the
  test or in the code.
- A live test that only checks "no error", or that logs a result
  instead of asserting on it.
- Fixtures generated at test time and never committed. A fixture that
  is not in the repository cannot catch a regression.
- Copying a fake or a helper into each test file. Shared fakes live in
  `tau-testing`, and a plugin's UI fakes in `tau_ui_plugin::testing`:
  `FakeRun` (the run a fold reaches), `run_ctx`, `fold` (a body through
  the registry, as the interface folds it) and `RepoValues`.
- One test that checks many unrelated rules. When it fails, the name
  should say which rule broke.
- Committing a `reproduce_failure` blob instead of a readable
  regression case.

## Review checklist

- [ ] Every new rule in the reference docs has a test that names it.
- [ ] Properties use an oracle from [Choosing a property](#choosing-a-property).
- [ ] Generators build valid values, and reach the special characters
      and boundaries the code handles.
- [ ] A shrunk counterexample is short enough to read.
- [ ] The test is deterministic: paused time, injected ids, no network.
- [ ] Case counts follow [Case counts](#case-counts); slow cases are in
      a nightly variant.
- [ ] Each fixed bug has a regression case, and relevant
      [known cases](#known-cases) are wired in.
- [ ] Protocol rules are tested on `ws::proto` directly; driver changes
      are tested in turmoil against `FakeOpenAi`, and assert on
      `PoolStats` as well as on output.
- [ ] Example tests are limited to the cases in
      [When to write an example test](#when-to-write-an-example-test).
