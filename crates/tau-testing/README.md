# tau-testing

Test support for tau: a scripted model, fake OpenAI and ChatGPT servers,
shared Hegel generators, and helpers that run async code from a synchronous
test. With it, an agent's tests run offline and deterministically, without
a ChatGPT sign-in or a network.

## What it provides

| Item                        | What it is                                                                 |
| --------------------------- | -------------------------------------------------------------------------- |
| `block_on`                  | Runs a future on a current-thread runtime with paused time                 |
| `block_on_io`               | Runs a future on this thread's runtime with real time and I/O              |
| `scripted::ScriptedModel`   | A deterministic `tau_ai::llm::Llm`, scripted turn by turn                  |
| `scripted::TurnBuilder`     | Builds one turn: text, thinking, tool calls, usage, cost, errors, delays   |
| `fake_openai::FakeOpenAi`   | A Responses WebSocket server that keeps OpenAI's continuation rules        |
| `fake_chatgpt::FakeChatGpt` | OpenAI's auth server and model list, for the Sign in with ChatGPT tests    |
| `generators`                | Shared Hegel generators: messages, transcripts, tool calls, schemas, usage |
| `generators::lane`          | Lane histories for the WebSocket delta rule                                |
| `openai`                    | Renders assistant messages as the `response.*` frames OpenAI would send    |
| `stream`                    | Renders messages as event streams, split at drawn points                   |
| `git`                       | Runs `git` for fixture repositories, with none of the user's settings      |

`ScriptedModel` is ported from pi's faux provider. Its clones share one
queue of turns and one record of requests. It answers with deltas split
the same way every time, simulates the prompt cache per session, and can
script failures: `TurnBuilder::error`, `dropped` and `fails_before_start`.
`ScriptedModel::requests` returns what each request was asked, and
`assert_exhausted` checks the whole script was used. When the script runs
out, it answers with an error and never hangs.

`FakeOpenAi` runs as a turmoil host (`install`) or on a local TCP listener
(`listen`). It records the input it rebuilt from its cache plus each delta,
which is the oracle for the delta rule, and lists the protocol rules a
client broke in `violations`. `FakeChatGpt` does the same for the OAuth
flow: it registers clients, signs ID tokens with the test keys in `data/`,
and rotates refresh tokens.

Use `block_on` for agent loops and anything driven by timers: sleeps
finish at once, and task order repeats exactly. Use `block_on_io` for real
I/O such as a SQLite file or a blocking pool. Never run real sockets under
`block_on`: paused time jumps ahead while the runtime waits on them.

## How it fits

It builds on `tau-ai`, whose `Llm` trait `ScriptedModel` implements. It is
a dev-dependency of `tau-ai`, `tau-agent`, `tau-store-sqlite`, `tau-ui` and
most plugin crates, and a normal dependency of `tau-codemode-eval` and
`tau-output-pruning-eval`. `tau-ai`'s transport and sign-in tests use the
fake servers; `tau-vcs-host`'s tests use `git`.

Since `tau-testing` depends on `tau-ai`, a `#[cfg(test)]` module inside
`tau-ai` or `tau-agent` cannot use it. Tests that use it go in the crate's
`tests/` directory.

## Usage

```rust
use tau_agent::agent::Agent;
use tau_testing::{block_on, scripted::ScriptedModel};

#[test]
fn answers_in_one_turn() {
    let model = ScriptedModel::new().turn(|t| t.text("Rayleigh scattering."));
    let agent = Agent::new(model.clone()).name("physicist");

    let outcome = block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        agent.run("Why is the sky blue?", &store).await.unwrap()
    });

    assert_eq!(outcome.text, "Rayleigh scattering.");
    assert_eq!(model.requests().len(), 1);
    model.assert_exhausted();
}
```

Tool calls are scripted the same way:

```rust
use serde_json::json;

let model = ScriptedModel::new()
    .turn(|t| t.thinking("...").tool_call("search", json!({"q": "tokio"})))
    .turn(|t| t.text("Found it.").usage(1200, 80));
```

## Testing

```sh
cargo nextest run --release -p tau-testing
```

`tests/scripted.rs` checks `ScriptedModel` itself: its event grammar,
recording, exhaustion, failures and cache simulation. nextest does not run
doc tests; the example in `src/scripted.rs` runs with
`cargo test --release --doc -p tau-testing`.

## Further reading

- [Testing](../../docs/reference/testing.md)
- [Sign in with ChatGPT](../../docs/reference/chatgpt-sign-in.md), "Tests"
- [OpenAI WebSocket](../../docs/reference/openai-websocket.md)
