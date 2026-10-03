# Public API

This is the target API. Names may still change during M2–M4. The
examples double as the acceptance tests for M4.

## Clients and store

```rust
let chatgpt = tau_ai::chatgpt::ChatGpt::new(tau_ai::chatgpt::Store::open_default()?);
let account = chatgpt.active()?.expect("signed in"); // an AccountId from a finished sign-in
let llm = tau_ai::client::OpenAi::chatgpt(chatgpt, account); // the plan; one WebSocket pool per client
let store = tau_store::Store::open("runs.db").await?;
let test_store = tau_store::Store::memory().await?;
```

`Agent::new` accepts anything implementing `tau_ai::llm::Llm`, by value.
Two types do: `OpenAi` and `tau_testing::ScriptedModel`; both are cheap to
clone and share their state between clones. `OpenAi::chatgpt` is the
only way to reach OpenAI: a ChatGPT plan through Sign in with ChatGPT,
with plan usage enabled. There is no API-key client
([0012](../decisions/0012-chatgpt-sign-in-only.md)). Tests build one with
`OpenAi::with_connector(connector, limits)` over a simulated network.
The client stops a run on a usage limit or a dead sign-in without
retrying; `OpenAi::refusal()` says why
([`chatgpt-sign-in.md`](chatgpt-sign-in.md)).

## Agent

```rust
pub struct Agent { /* Arc inside; Clone is cheap; immutable once built */ }

impl Agent {
    pub fn new(llm: impl Llm) -> Self;
    pub fn name(self, name: &str) -> Self;
    pub fn model(self, id: &str) -> Self;
    pub fn instructions(self, text: impl Into<String>) -> Self;
    pub fn reasoning(self, effort: ReasoningEffort) -> Self;
    pub fn tool(self, t: impl AgentTool) -> Self;
    pub fn tools(self, ts: impl IntoIterator<Item = Arc<dyn AgentTool>>) -> Self;  // e.g. tau_tools::coding_tools(&root)
    pub fn plugin(self, p: impl Plugin) -> Self;      // e.g. tau_compaction::Compaction, tau_tools::plugin::CodingTools
    pub fn limits(self, l: Limits) -> Self;
    pub fn retry(self, p: RetryPolicy) -> Self;       // 3 attempts, 2 s base
    pub fn warmup(self, on: bool) -> Self;            // generate:false on first use
    pub fn clock(self, clock: Clock) -> Self;         // message timestamps; for deterministic tests

    pub fn start(&self, input: impl Into<Input>, store: &Store) -> Run;
    pub async fn run(&self, input: impl Into<Input>, store: &Store) -> Result<Outcome>;
    pub async fn run_typed<T>(&self, input: impl Into<Input>, store: &Store) -> Result<Typed<T>>
    where T: DeserializeOwned + JsonSchema;
    pub fn as_tool(&self, name: &str, description: &str) -> SubAgent; // SubAgent: AgentTool
    pub fn fork(&self, from: &Checkpoint) -> Forked;  // Forked::run / run_typed / start
    pub fn resume(&self, run: &RunId) -> Resumed;    // a finished run goes on, in place: Resumed::run / start
}
```

`Input` is the run's first user message, and optionally the workflow it
belongs to. Strings convert into it.

```rust
pub struct Input { pub text: String, pub workflow: Option<String> }
impl Input {
    pub fn new(text: impl Into<String>) -> Self;
    pub fn workflow(self, id: impl Into<String>) -> Self;  // groups runs for Store::workflow_cost
}
```

Sub-agent runs inherit their parent's workflow. Forks do too, unless
their input names another.

A sub-agent starts blank: it sees only its input. `SubAgent::forking`
makes each call fork the calling run instead. The sub-agent then sees
the caller's stored transcript, the turn that made the call, an output
for each call in that turn (its own, a sibling sub-agent's, or another
tool's, whose result it cannot see), and then its input. `delegate`
works this way ([0015](../decisions/0015-delegates-fork-their-caller.md)).

Every run's requests carry a `Lineage` in their `Settings`: the run's
id as its path, sent as `prompt_cache_key`, and for a fork or a forking
sub-agent that has not been resumed, the id of the run it forks, when
that run's last turn was on the same model. The transport puts the
run's requests on a connection that already serves its path, or a
fork's first request on its parent's
([openai-websocket.md](openai-websocket.md), "Prompt cache"). A resumed
run keeps its path, so it goes back to its connection.

## Run and Outcome

```rust
impl Run {
    pub fn id(&self) -> RunId;
    pub fn events(&mut self) -> impl Stream<Item = RunEvent> + '_;
    pub fn steer(&self, msg: impl Into<String>);
    pub fn cancel(&self);
    pub async fn outcome(self) -> Result<Outcome>;
}

pub struct Outcome {
    pub run: RunId,
    pub text: String,
    pub stop: StopReason,        // Stop | Limit(LimitKind) | Cancelled | Error
    pub usage: Usage,            // includes child runs
    pub cost: Cost,
}
impl Outcome { pub fn checkpoint(&self) -> Checkpoint; }

pub struct Typed<T> { pub value: T, pub outcome: Outcome }
impl<T: Serialize> Typed<T> { pub fn json(&self) -> String; }
```

## Tools

```rust
#[async_trait]
pub trait AgentTool: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &serde_json::Value;           // rewritten to OpenAI strict form
    fn execution_mode(&self) -> ExecutionMode { ExecutionMode::Parallel }
    fn exposure(&self) -> Exposure { Exposure::Direct }      // Direct, Nested or ModelOnly
    fn output_schema(&self) -> Option<&Value> { None }       // the shape of `structured`
    fn prepare_arguments(&self, raw: Value) -> Value { raw }
    async fn call(&self, args: Value, ctx: ToolCtx) -> Result<ToolOutput, ToolError>;
}

pub struct ToolOutput {
    pub content: Vec<InputBlock>,     // what the model sees
    pub details: Option<Value>,       // for interfaces and plugins
    pub structured: Option<Value>,    // for a calling tool
}

#[async_trait]
pub trait TypedTool: Send + Sync + 'static {
    type Args: DeserializeOwned + JsonSchema + Send;
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    async fn call(&self, args: Self::Args, ctx: ToolCtx) -> Result<ToolOutput, ToolError>;
}

pub struct ToolCtx { pub cancel: CancellationToken, pub updates: ToolUpdates, pub run: RunId }
// ToolCtx::call(name, args), catalog() and plugin(): tools that call
// tools, in plugins.md, "Nested calls".
```

A tool's error is a `ToolError` (`tau_agent::error`, re-exported from
`tool`). `?` converts io, JSON and `JoinError`s, a sub-agent's error,
and a `String` or `&str` message into it; anything else goes in with
`ToolError::other`. The model reads its `Display` as the tool result.
`ToolError::output(ToolOutput)` fails with a whole output: its text is
the error the model reads, and its `details` stay on the result, as
`bash` does for a command that exits non-zero.
Plugins return a `PluginError` the same way, which also
converts store errors and `PluginCtx::ask`'s `AskError`; the loop
reports it as `RunEvent::PluginError`, with its chain of sources.

## Plugins

An agent is extended only through `Plugin` and `PluginRun`, in
`tau_agent::plugin` ([`plugins.md`](plugins.md)). The tool call a plugin
sees and its answer live there too:

```rust
pub struct ToolCall { pub id: String, pub name: String, pub args: Value, pub parent: Option<String> }
pub enum Decision { Allow, Block(String) }   // a Block's reason is the result the model sees
```

## Limits

```rust
Limits::default()
    .max_turns(30)
    .max_tokens(400_000)
    .max_usd(2.0)
    .timeout(Duration::from_secs(900))
```

Limits are checked after every turn against actual usage. Child runs
started through `as_tool` count toward their parent's limits.

## Typed results

`run_typed::<T>` produces the model's answer as a value of type `T`:

1. It derives the schema of `T` with `schemars`.
2. It rewrites that schema into OpenAI's strict form: every property
   required, `additionalProperties: false`, and optional fields as
   nullable. The rewrite is ported from pi's `constrained-sampling.ts`,
   with two additions for Rust types: references to `$defs` are
   inlined first, and nullable objects (`Option<Struct>`) are allowed.
   A recursive type has no strict form and fails with
   `AgentError::OutputSchema` before the model is asked anything.
3. It sends the result as `text: { format: { type: "json_schema", name,
schema, strict: true } }`.

Tools stay available during a typed run. Only the final message must
match `T`. A final message that does not parse fails the run with
`AgentError::Output`, which keeps the `Outcome`.

## Examples

### Typed pipeline with parallel steps

```rust
#[derive(Deserialize, JsonSchema)]
struct Changes { features: Vec<String>, fixes: Vec<String>, breaking: Vec<String> }
#[derive(Deserialize, JsonSchema)]
struct Review { approved: bool, problems: Vec<String> }

let changes = scanner.run_typed::<Changes>("v1.4.0..v1.5.0", &store).await?;
let (terse, detailed) = tokio::try_join!(
    writer.run(format!("Terse style.\n{}", changes.json()), &store),
    writer.run(format!("Detailed style.\n{}", changes.json()), &store),
)?;
for draft in [&terse, &detailed] {
    let review = reviewer
        .run_typed::<Review>(format!("{}\n---\n{}", changes.json(), draft.text), &store)
        .await?;
    if review.value.approved { return Ok(draft.text.clone()); }
}
```

### Supervisor with sub-agents

```rust
let lead = Agent::new(llm.clone()).name("lead").model("gpt-5.5")
    .instructions("Break the task down. Delegate. Verify before finishing.")
    .tool(researcher.as_tool("research", "Investigate a question and report findings."))
    .tool(coder.as_tool("implement", "Make a scoped code change and report what changed."))
    .limits(Limits::default().max_usd(5.0).timeout(Duration::from_secs(1800)));

let mut run = lead.start("Add retry with jitter to the HTTP client.", &store);
while let Some(ev) = run.events().next().await {
    match ev {
        RunEvent::ToolStart { run, tool, .. } => eprintln!("[{run}] → {tool}"),
        RunEvent::TextDelta { parent: None, delta, .. } => print!("{delta}"),
        _ => {}
    }
}
let outcome = run.outcome().await?;
```

### Fork fan-out

```rust
let investigation = debugger.run("Find the root cause. Don't fix it yet.", &store).await?;
let base = investigation.checkpoint();
let attempts = ["minimal fix", "fix plus regression test", "refactor clock injection"]
    .map(|s| debugger.fork(&base).run(format!("Now implement: {s}"), &store));
let results = futures::future::join_all(attempts).await;
```

### Guard plugin and test

```rust
struct NoProdWrites;
#[async_trait]
impl Plugin for NoProdWrites {
    fn name(&self) -> &str { "no-prod-writes" }
    async fn start(&self, _: &mut RunPlan, _: &PluginCtx) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(NoProdWritesRun))
    }
}
struct NoProdWritesRun;
#[async_trait]
impl PluginRun for NoProdWritesRun {
    async fn before_tool(&mut self, call: &mut ToolCall, _: &PluginCtx) -> Result<Decision, PluginError> {
        let prod = call.args["command"].as_str().is_some_and(|c| c.contains("--env prod"));
        if call.name == "bash" && prod { return Ok(Decision::Block("no production commands".into())); }
        Ok(Decision::Allow)
    }
}

#[tokio::test]
async fn triage_opens_one_ticket() {
    let llm = tau_testing::ScriptedModel::new()
        .turn(|t| t.tool_call("create_ticket", json!({"title": "Crash on start", "body": "…"})))
        .turn(|t| t.text("Opened one ticket."));
    let tracker = FakeTracker::default();
    let agent = Agent::new(llm).tool(typed(CreateTicket(tracker.clone()))).plugin(NoProdWrites);
    agent.run("Triage: …", &Store::memory().await.unwrap()).await.unwrap();
    assert_eq!(tracker.created().len(), 1);
}
```
