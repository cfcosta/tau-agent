# tau-ask

Lets an agent ask the person structured questions and wait for the
answers. The `ask` tool takes one to four questions, each with two to
four choices, one or several to pick, and a preview per choice. The
person can always write their own answer instead, and add a note to any
answer. The call stays open until the answers come, the person
declines, or the run is cancelled. The answers are the call's result,
so the turn goes on without a new message.

## What it provides

| Item                                         | What it is                                                                 |
| -------------------------------------------- | -------------------------------------------------------------------------- |
| `Ask`, `Question`, `Choice`                  | What the agent asks; `Ask::check` enforces the limits                      |
| `Reply`, `Answer`                            | How the person answered (`Answered` or `Declined`); `Reply::check` against the `Ask` |
| `ask::MAX_QUESTIONS`, `MIN_CHOICES`, ...     | The limits, and `OTHER`, the label no choice may take                      |
| `Record`                                     | What the plugin publishes: `Asked`, `Answered`, `Closed`                   |
| `NAME`, `TOOL`                               | `"tau-ask"` and `"ask"`                                                    |
| `AskUi`                                      | The plugin's `UiPlugin`: the panel in the composer's place, and the card   |
| `ui::State`, `ui::Draft`                     | The fold of a run's calls; the person's answers in progress and their keys |
| `host::Waiting`                              | The calls waiting for an answer; `Waiting::answer` gives one its reply (feature `host`) |
| `host::AskPlugin`, `host::AskTool`           | The agent plugin and the tool (feature `host`)                             |
| `AskHost`                                    | The `HostHalf` for tau's plugin registry (feature `host`)                  |

`AskPlugin::refusing()` is for sub-agents, which have no one to ask:
the tool is still declared, so the run's tools match its caller's and
it can read their prompt cache, but every call fails with
`NO_ONE_TO_ASK`. When a run starts, the plugin closes the calls its
history left waiting.

## How it fits

tau-ask is a single crate holding both halves. The shapes, the fold and
the views always build. The tool and the agent plugin sit behind the
default `host` feature, so `tau-ui-remote` and a phone build it with
`default-features = false` (decisions 0017, 0030).

- Builds on `tau-agent` (`Plugin`, `AgentTool`, `PluginCtx::publish`),
  `tau-ui-plugin` and `tau-ui-kit`.
- Used by `tau-ui`, which registers `AskHost`, and `tau-ui-remote`,
  which lists `AskUi`. The panel is a contribution at
  `tau_ui_plugin::points::COMPOSER`, drawn in the composer's place.

## Usage

Outside tau's interface, a program answers the questions itself. It
watches the run's events for an `Asked` record, then replies through
the same `Waiting` the plugin holds:

```rust
use futures_util::StreamExt;
use tau_agent::{agent::Agent, event::RunEvent};
use tau_ask::{NAME, Record, Reply, host::{AskPlugin, Waiting}};

let waiting = Waiting::default();
let agent = Agent::new(llm).plugin(AskPlugin::new(waiting.clone()));

let mut run = agent.start("Plan the migration", &store);
let id = run.id();
let mut events = run.events();
while let Some(event) = events.next().await {
    if let RunEvent::PluginReport { plugin, body, .. } = &event
        && &**plugin == NAME
        && let Some(Record::Asked { call, ask }) = Record::parse(body)
    {
        // Show `ask` to the person; here, decline.
        waiting.answer(&id.0, &call, Reply::Declined)?;
    }
}
```

`Waiting::answer` refuses a reply that does not fit what was asked, and
the call goes on waiting.

## Features

| Feature | Default | Effect                                                          |
| ------- | ------- | --------------------------------------------------------------- |
| `host`  | on      | `host`, `AskHost` and the tool's JSON schema, with schemars and tokio |

## Testing

```sh
cargo nextest run --release -p tau-ask
```

- `tests/ask.rs` holds Hegel property tests: questions within the
  limits are asked and any one fault is refused; however the person
  works the panel, what it sends answers what was asked.
- `tests/run.rs` (feature `host`) runs `ask` inside real runs with
  `ScriptedModel`, the test playing the person: answers, cancels,
  wrong answers, and calls a later run closes.
- `tests/ui.rs` (feature `host`) checks that every run gets `ask` and
  that a sub-agent's refuses every call.

## Further reading

- [tau-ask reference](../../../docs/reference/ask.md)
- [0019: The agent asks the person, in the composer's place](../../../docs/decisions/0019-ask-the-person.md)
- [0022: The prompt cache follows the connection](../../../docs/decisions/0022-the-prompt-cache-follows-the-connection.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
