# Testing

This document sets the testing strategy for tau-agent and the quality bar
every test must meet. It covers:

- the rules that make a test worth keeping;
- property-based testing with Hegel, which is the default for new tests;
- the property inventory for each crate;
- the test harness: `ScriptedModel`, recorded streams and sqlx;
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
   `OPENAI_API_KEY`, `DATABASE_URL` or network access.

## Tools

| Tool                           | Use                                                                    |
| ------------------------------ | ---------------------------------------------------------------------- |
| `hegeltest` (lib name `hegel`) | Property-based tests: generators, shrinking, stateful model tests      |
| `cargo nextest`                | Test runner, locally and in CI                                         |
| `tau-testing`                  | `ScriptedModel`, shared generators, recorded-stream replay, async glue |
| `tokio::time::pause`           | Deterministic time for retries, timeouts, idle timers and rotation     |
| `tempfile`                     | Filesystem fixtures for `tau-tools`                                    |
| `cargo mutants`                | Test strength on the core modules                                      |
| `cargo sqlx prepare --check`   | The committed `.sqlx/` metadata matches the queries                    |

All of these are in the dev shell, except the crates, which come
through Cargo. `hegeltest` is declared once in
`[workspace.dependencies]`. It is a dev-dependency of every crate that
has tests, and a normal dependency of `tau-testing`, which exports the
shared generators.

Hegel is used rather than `proptest` because it shrinks through
Hypothesis's engine. That engine keeps generated values valid while it
shrinks, so generators need no custom shrink logic. Hegel also has
stateful model testing built in.

## Layers

| Layer         | What it covers                                                                        | Harness                                                          |
| ------------- | ------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| Property      | Pure logic: delta rule, conversions, parsers, schema rewrites, truncation, cut points | Hegel; `tests/` for public APIs, `#[cfg(test)]` for private code |
| Model         | Stateful components: WebSocket pool and lanes, the store, the agent loop              | `#[hegel::state_machine]` against a simple in-memory model       |
| Replay        | `tau-ai` event processing against recorded `response.*` streams                       | fixtures under `crates/tau-ai/tests/fixtures/`                   |
| Scripted      | Workflow behaviour: typed results, sub-agents, forks, limits, hooks                   | `ScriptedModel` + `Store::memory()`, generated scripts           |
| Live (opt-in) | The real WebSocket endpoint                                                           | `OPENAI_API_KEY` and `--features live`; never in default CI      |
| Mutation      | Strength of the tests on the delta rule, the loop and the store                       | `cargo mutants`                                                  |

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
| Extended variant (nightly only)        | 5,000–10,000      |

An extended variant is a second test with the same body, a higher
count, and `#[ignore = "extended"]`. The nightly job runs it with
`cargo nextest run --run-ignored only`. Put the body in a plain function
so both tests share it.

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

### Stateful model tests

Use `#[hegel::state_machine]` for the pool, the lanes, the store and the
loop:

- Each `#[rule]` performs one operation on both the real component and
  the model, and compares the results.
- Each `#[invariant]` checks a rule that must hold after every step.
- Guard a rule with `tc.assume` only for preconditions such as "the pool
  has a free lane". Such rules must still apply in most states.
- Use `hegel::stateful::Variables` for handles created earlier in the
  run, such as run ids, lanes and checkpoints.
- Raise `stateful_step_count` only in extended variants.

### Failures and regressions

- The local example database is `.hegel/` in the workspace root. It is
  gitignored. Hegel replays saved failures first on the next run.
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

| Case                                      | Why                                                                |
| ----------------------------------------- | ------------------------------------------------------------------ |
| Exact strings from [`tools.md`](tools.md) | Models are tuned to them; the exact text is the spec               |
| Wire shapes pinned by pi or OpenAI        | A golden JSON fixture is the clearest statement of the shape       |
| Recorded streams                          | They are real server output, which a generator only approximates   |
| Regressions from a shrunk counterexample  | They keep a found bug found                                        |
| The API examples in [`api.md`](api.md)    | They are M4's acceptance tests and documentation                   |
| Table-driven classification               | Retry codes and HTTP statuses: the table is finite; test all of it |

Everything else starts as a property.

## Property inventory

Each crate's properties are listed below, with the oracle for each.
Implement a property in the same milestone as the code it covers. The
list is a floor, not a ceiling.

### `tau-ai`

| Property                                                                                                                                                                            | Oracle       |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| Message types: `from_json(to_json(m)) == m`, and the JSON uses pi's shape (camelCase, tagged by `role` / `type`)                                                                    | Round trip   |
| `Accumulator` over any `AssistantEvent` stream built from a message gives back that message                                                                                         | Round trip   |
| Re-chunking the deltas of a stream at arbitrary points does not change the accumulated message                                                                                      | Metamorphic  |
| Event processor: render a generated response as `response.*` events, with arbitrary delta splits and interleaved lanes; processing gives back the response                          | Round trip   |
| Every processed stream starts with `start` and ends with exactly one of `done` or `error`                                                                                           | Invariant    |
| Incremental JSON parser, fed the arguments in any chunking, agrees with `serde_json` on the complete text                                                                           | Differential |
| Each partial parse is consistent with the final value: no field seen early is later changed or dropped                                                                              | Invariant    |
| Transcript → Responses input: tool-call ids split into `call_id` and `item_id`, and every tool output follows its call                                                              | Invariant    |
| **Delta rule:** for any lane history, `baseline + sent_input` equals the full input the request would otherwise carry                                                               | Differential |
| Delta rule: a change to anything other than `input`, or to the baseline prefix, forces a full resend with no `previous_response_id`                                                 | Metamorphic  |
| Cancel, compaction, reconnect and `previous_response_not_found` each make the lane's next request a full resend                                                                     | Model        |
| Pool (state machine): never more than 32 named lanes or 16 in-flight responses per connection; requests on a lane stay FIFO; no new work goes to a connection older than 55 minutes | Model        |
| Backoff delay for attempt `n` lies in `[0, base × 2ⁿ]`, and the number of attempts never exceeds the limit                                                                          | Invariant    |
| Cost is additive: `cost(a + b) == cost(a) + cost(b)` for usages of the same model and tier                                                                                          | Algebraic    |

The delta rule is the riskiest code in the project. Its model test must
also run in the nightly extended tier and under `cargo mutants`.

### `tau-agent`

| Property                                                                                                                    | Oracle      |
| --------------------------------------------------------------------------------------------------------------------------- | ----------- |
| Strict schema rewrite: every object in the output has all properties required and `additionalProperties: false`             | Invariant   |
| Strict schema rewrite is idempotent                                                                                         | Algebraic   |
| A value valid under the original schema, with missing optional fields set to `null`, is valid under the strict schema       | Metamorphic |
| Coercion leaves a value that already validates unchanged                                                                    | Invariant   |
| Coercion is idempotent                                                                                                      | Algebraic   |
| A number, a boolean or a one-element array gives the same result as its string or scalar form after coercion                | Metamorphic |
| `before_tool` hooks: the result equals a left fold over the hooks in which the first `Block` or error wins                  | Model       |
| **Loop, over generated scripts** (tool calls, tool results that succeed, fail or take time, steering, cancel at any point): | Model       |
| · every tool call in the transcript has exactly one result, including after a cancel                                        |             |
| · result messages are in source order; `ToolStart` is in source order; `ToolEnd` is in completion order                     |             |
| · the event stream matches the grammar `RunStart (TurnStart … TurnEnd)* RunEnd`, and nothing follows `RunEnd`               |             |
| · steered messages appear after the tool batch that was running, in the order they were sent                                |             |
| · the persisted usage equals the sum of the turns' usage                                                                    |             |
| Limits: a run ends with `StopReason::Limit` if and only if a limit was exceeded after some turn, and child usage counts     | Model       |
| A fork's transcript equals the parent's transcript up to the checkpoint, followed by the fork's own messages                | Model       |
| Compaction cut point never falls between a tool call and its result, and never on a tool result                             | Invariant   |
| The kept suffix holds at least `keep_recent_tokens`, unless the whole transcript holds fewer                                | Invariant   |
| The token estimate never decreases when a message is appended                                                               | Invariant   |

### `tau-store`

| Property                                                                                                              | Oracle       |
| --------------------------------------------------------------------------------------------------------------------- | ------------ |
| State machine over create run, append turn, fork, compact and finish, against a `Vec`-based model; every read matches | Model        |
| After any sequence of appends, a run's `seq` values are `0..n` with no gaps                                           | Invariant    |
| The recursive-CTE transcript equals a Rust walk up the fork chain over the model                                      | Differential |
| Loading a run drops everything before its latest compaction record                                                    | Model        |
| A failed append leaves neither messages nor usage totals behind                                                       | Model        |
| Message bodies round-trip through the `body` column unchanged                                                         | Round trip   |

Store tests use `Store::memory()`. Each Hegel case opens a new store, so
cases do not share state.

### `tau-tools`

| Property                                                                                                        | Oracle       |
| --------------------------------------------------------------------------------------------------------------- | ------------ |
| `truncate_head` output is a prefix of whole lines, within `MAX_LINES` and `MAX_BYTES`                           | Invariant    |
| `truncate_tail` output is a suffix, within the limits, and valid UTF-8 at every cut                             | Invariant    |
| Truncation is idempotent, and input already within the limits is returned unchanged                             | Algebraic    |
| `edit` with exact, unique, non-overlapping edits equals a naive reference that applies them to the string       | Differential |
| `edit` on a CRLF file, or a file with a BOM, equals the LF edit with the line ending and BOM restored           | Metamorphic  |
| In fuzzy mode, lines that no edit touches keep their original bytes                                             | Invariant    |
| `edit` rejects overlapping edits and edits that match more than once, in the space where they matched           | Invariant    |
| `grep` over a generated tree equals a naive regex scan of each file's lines, including lines with U+2028/U+2029 | Differential |
| `find` equals `globset` matching over a naive walk; "limit reached" appears only when more results existed      | Differential |
| `ls` output is sorted case-insensitively, with a trailing `/` on directories and dotfiles included              | Invariant    |
| `read` with `offset` and `limit` equals the same slice of the file's lines                                      | Differential |

`bash` is tested with example tests plus one property: for generated
output sizes, the tail kept equals `truncate_tail` of the full output,
and the spill file holds the full output. Process-group behaviour is
checked by example tests that start a grandchild process.

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
- two lanes interleaved on one socket.

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
- `tau-agent`: the loop, coercion and the strict schema rewrite;
- `tau-store`: the append and transcript queries.

A surviving mutant means a behaviour no test checks. Either add the
missing assertion, or record why the mutant is equivalent in
`.cargo/mutants.toml` with a comment.

## CI tiers

| Tier    | When       | Runs                                                                                                                             |
| ------- | ---------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Check   | every push | `nix fmt` check, `cargo clippy --all-targets -D warnings`, `cargo nextest run`, `cargo sqlx prepare --check`, `cargo deny check` |
| Nightly | once a day | everything in Check, plus `cargo nextest run --run-ignored only` (extended properties) and `cargo mutants` on the modules above  |
| Live    | by hand    | `cargo nextest run --features live` with `OPENAI_API_KEY`; records new fixtures when the protocol changes                        |

The Check tier must stay under five minutes. If it grows past that,
move cases to extended variants rather than lowering the default counts.

## Anti-patterns

- A property that recomputes the result the same way the code does.
- A test whose only assertion is that nothing panicked.
- Generating random input and discarding most of it with `tc.assume`.
- Real sleeps, real clocks, `thread_rng`, or `HashMap` iteration order
  in an assertion.
- Asserting on `Debug` output or on error text, except for the pinned
  strings in [`tools.md`](tools.md).
- Mocking the component under test. Mock only the edges: the LLM
  (`ScriptedModel`), the clock (paused tokio time), and the filesystem
  (a tempdir).
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
      an extended variant.
- [ ] Each fixed bug has a regression case.
- [ ] Example tests are limited to the cases in
      [When to write an example test](#when-to-write-an-example-test).
