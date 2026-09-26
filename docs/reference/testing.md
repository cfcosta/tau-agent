# Testing

## Layers

| Layer         | What it covers                                                                                | Tools                                                                |
| ------------- | --------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| Unit          | Delta rule, strict-schema rewriting, coercion, truncation, edit matching, cut-point selection | `cargo nextest`, property tests for the delta rule and edit matching |
| Replay        | `tau-ai` event processing against recorded `response.*` streams                               | fixtures under `crates/tau-ai/tests/fixtures/`                       |
| Scripted      | Agent loop and workflow logic, with no network                                                | `tau-testing::ScriptedModel` + `Store::memory()`                     |
| Live (opt-in) | The real WebSocket endpoint                                                                   | `OPENAI_API_KEY` set and `--features live`; never in default CI      |
| Mutation      | Test strength on the delta rule, the loop and the store                                       | `cargo mutants` (already in the dev shell)                           |

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

## Replaying stored runs

`tau-testing::replay(store, run_id)` turns a stored run into a
`ScriptedModel` script. A real run can then become a regression test for
the workflow code around it.

## sqlx in tests

- `Store::memory()` runs the migrations on `sqlite::memory:`.
- The query macros check against the committed `.sqlx/` metadata, so
  tests need no `DATABASE_URL`.
- CI runs `cargo sqlx prepare --check`.
