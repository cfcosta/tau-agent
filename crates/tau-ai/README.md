# tau-ai

The model layer of tau: OpenAI's Responses API over its WebSocket mode,
paid by the user's ChatGPT plan through Sign in with ChatGPT. It holds the
transcript and event types, the connection pool, the delta rule that
continues a conversation on the same connection, retry classification,
the model table and cost. There are no API keys and no HTTP transport for
inference.

Much of it is ported from pi (`packages/ai`); the module docs cite the pi
file and line each part comes from.

## What it provides

| Module         | What it holds                                                                             |
| -------------- | ----------------------------------------------------------------------------------------- |
| `client`       | `OpenAi`, the client; `Session`, one run's conversation; `SessionResponse`, its events    |
| `llm`          | The `Llm` and `LlmSession` traits the agent loop runs against                             |
| `chatgpt`      | Sign in with ChatGPT: `ChatGpt`, its `Store` of accounts, `Loopback`, token refresh       |
| `message`      | Transcript types: `Message`, `UserMessage`, `AssistantMessage`, `ToolCall`, `Usage`       |
| `event`        | `AssistantEvent`, the streamed events of one response, and the `Accumulator`              |
| `responses`    | Transcript to Responses `input`, the `response.create` body and `Settings`, frame parsing |
| `ws`           | The transport: `proto` (pool and lane state machines, no I/O) and `io` (the driver)       |
| `retry`        | `classify`, `RetryPolicy` and `Recovery`: what to retry, and how long to wait             |
| `refusal`      | `Refusal`: why OpenAI or the sign-in refused, kept as it came                             |
| `model`        | The OpenAI model table: `find`, `models`, `plan_models`, prices and limits                |
| `cost`         | `cost` and `apply`: a usage's price, with cache reads and the long-context tier           |
| `partial_json` | `PartialJson`, which parses streamed tool-call arguments as they arrive                   |
| `http`         | Just enough HTTP/1.1 for OAuth and a few JSON endpoints, over a `Dialer`                  |
| `files`        | Writes files only their owner may read: credentials and keys                              |
| `time`         | RFC 3339 times in UTC, without a date library                                             |

`OpenAi` starts one WebSocket transport, and its clones share it, so
create one client and clone it. It must be created inside a tokio runtime.
Each `Session` gets a lane: a connection that already serves its
conversation, or its parent's for a fork, so the prompt cache follows it.
When a request only adds to the last one, the lane sends a delta with
`previous_response_id`; anything else is a full resend. Dropped
continuations and connection-limit errors are recovered inside the
transport, and the caller never sees them.

The model table is built from models.dev's catalog in
`data/models-dev-openai.json` plus the corrections in
`data/openai-overrides.json`. `scripts/update-openai-models.sh` refreshes
the catalog.

## How it fits

`tau-ai` depends on no other tau crate. `tau-agent` runs its loop against
`Llm`, and `OpenAi` is the implementation it gets in production;
`tau_testing::ScriptedModel` is the one it gets in tests. `tau-ui`,
`tau-ui-plugin`, `tau-ui-remote`, `tau-remote`, the evals and most plugin
crates use its message and model types.

## Usage

Most programs reach the model through `tau_agent::agent::Agent`. To talk
to it directly:

```rust
use tau_ai::{
    chatgpt::{self, ChatGpt},
    client::OpenAi,
    event::AssistantEvent,
    message::{Message, UserContent, UserMessage},
    responses::request::Settings,
};

// A saved Sign in with ChatGPT, with plan usage enabled.
let chatgpt = ChatGpt::new(chatgpt::Store::open_default()?);
let account = chatgpt.active()?.ok_or("sign in with ChatGPT first")?;
let llm = OpenAi::chatgpt(chatgpt, account);

let mut session = llm
    .session(Settings {
        model: "gpt-5.5".into(),
        instructions: Some("Answer in one sentence.".into()),
        ..Default::default()
    })
    .await?;
let transcript = [Message::User(UserMessage {
    content: UserContent::Text("Why is the sky blue?".into()),
    timestamp: 0,
})];
let mut response = session.respond(&transcript, 0);
while let Some(event) = response.next().await {
    match event {
        AssistantEvent::TextDelta { delta, .. } => print!("{delta}"),
        AssistantEvent::Done { usage, .. } => {
            println!("\n${:.4}", usage.cost.total);
        }
        _ => {}
    }
}
```

`chatgpt::Store::open_default` reads the sign-ins in
`$XDG_CONFIG_HOME/tau/chatgpt`. Signing in is `ChatGpt::start_sign_in`,
then `ChatGpt::finish_sign_in` with the callback a `Loopback` listener
receives.

## Testing

```sh
cargo nextest run --release -p tau-ai
```

The tests run offline. Transport and sign-in tests run the real client in
a turmoil simulation against `FakeOpenAi` and `FakeChatGpt` from
`tau-testing`. Hegel properties cover the delta rule, the pool and lane
state machines, the stream processor, the partial JSON parser, retry and
cost. Deeper runs of the pool, lane, continuation and transport properties
are ignored by default:

```sh
cargo nextest run --release -p tau-ai --run-ignored only
```

Two benchmarks measure the transport and input building, sized by
environment variables (`RUNS`, `TURNS`, `PAYLOAD`, `DELTAS`, `THINK_MS`):

```sh
cargo bench -p tau-ai --bench transport
cargo bench -p tau-ai --bench input
```

Two examples are live probes, run by hand against `api.openai.com`. They
need a ChatGPT sign-in, which `chatgpt_probe -- sign-in` saves:

```sh
cargo run -p tau-ai --example chatgpt_probe -- accounts
cargo run -p tau-ai --example cache_probe -- --cases
```

## Further reading

- [OpenAI WebSocket](../../docs/reference/openai-websocket.md)
- [Sign in with ChatGPT](../../docs/reference/chatgpt-sign-in.md)
- [Testing](../../docs/reference/testing.md)
- [pi audit](../../docs/reference/pi-audit.md)
- [Architecture](../../docs/architecture.md), "I/O boundary"
- [Decision 0002: OpenAI only, Responses over WebSocket](../../docs/decisions/0002-openai-websocket-only.md)
- [Decision 0011: Sign in with ChatGPT](../../docs/decisions/0011-sign-in-with-chatgpt.md)
- [Decision 0012: Sign in with ChatGPT only](../../docs/decisions/0012-chatgpt-sign-in-only.md)
- [Decision 0022: The prompt cache follows the connection](../../docs/decisions/0022-the-prompt-cache-follows-the-connection.md)
