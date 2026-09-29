# Plugins

- Status: implemented in `tau_agent::plugin`; compaction is a plugin.
  The first four plugins are not built yet.
- Date: 2026-09-28

A plugin extends an agent from its own crate. It can add tools, shape a
run before it starts, check tool calls, rewrite the context between
turns, hold the run back from stopping, and act when the run ends.

This document defines those seams and checks them against the first
four plugins.

## Principles

- **Plugins are crates, linked at compile time.** tau-agent is a
  library ([0001](../decisions/0001-library-not-product.md)). There is
  no dynamic loading, no discovery, no settings file and no command
  surface. A plugin is a value that the caller builds and passes to
  `Agent::plugin`.
- **The delta rule decides where each seam sits.** A run's
  instructions, tools and reasoning are fixed for the whole run, and any
  edit to the transcript forces one full resend
  ([openai-websocket.md](openai-websocket.md)). So:
  - settings change in exactly one place, before the run opens its
    session;
  - a context rewrite replaces the working transcript and is stored,
    and it is not re-applied as a per-turn view (pi's `context` event).
    One rewrite costs one full resend, and the turns after it are deltas
    again;
  - every other seam leaves the request alone.
- **The run is the unit of state.** A plugin is shared by every run of
  an agent. For each run it creates a `PluginRun`, which holds that
  run's state and is called with `&mut self`, one call at a time. No
  plugin needs a lock keyed by run id.
- **Plugins pay their own way.** A plugin that calls a model reports the
  usage, and the usage counts toward the run's limits and outcome, as a
  sub-agent's does.
- **Built-ins use the same seams.** Compaction becomes the first plugin
  of the context seam. If the built-in cannot be written against a seam,
  the seam is wrong.

## The interfaces

### `Plugin`: one per agent

```rust
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// Names the plugin in events, errors and stored records.
    fn name(&self) -> &str;

    /// Tools the plugin adds to the agent. Read once, by `Agent::plugin`.
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        Vec::new()
    }

    /// Prepares one run and returns the plugin's state for it. Runs
    /// before the run opens its session, in registration order, so a
    /// later plugin sees what an earlier one set. An error fails the run
    /// with `AgentError::Plugin`.
    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Box<dyn PluginRun>>;
}
```

`Agent::plugin(p)` adds `p.tools()` to the agent and keeps `p`.
`Agent::hook(h)` stays: a `RunHook` becomes a plugin whose runs share
the one hook.

### `RunPlan`: the only place settings change

```rust
pub struct RunPlan {
    /// The user's input. A plugin may rewrite it.
    pub input: String,
    /// Context put in front of the input, in one user message, in
    /// plugin order. The instructions stay the same across runs, so
    /// OpenAI's prompt cache still hits them.
    pub context: Vec<String>,
    pub instructions: Option<String>,
    pub reasoning: Option<ReasoningEffort>,
    // Read-only, through methods:
    // model(), kind() (root, fork or sub-agent), workflow(),
    // records(): this plugin's records along the fork chain, oldest
    //   first (see `PluginCtx::record`);
    // last_rewrite(): the details of the latest context rewrite the run
    //   inherits, when this plugin made it.
}
```

After every `start` has run, the loop builds the settings from the plan
and opens the session. Nothing changes them afterwards.

### `PluginRun`: one per plugin per run

Every method has a default that does nothing, so a plugin implements
only what it uses. The loop calls plugins in registration order.

```rust
#[async_trait]
pub trait PluginRun: Send {
    /// `RunHook::before_tool`: may change the arguments (validated
    /// again) or block the call. The first block wins.
    async fn before_tool(&mut self, call: &mut ToolCall, ctx: &PluginCtx)
        -> anyhow::Result<Decision> { Ok(Decision::Allow) }

    /// `RunHook::after_tool`: may change the output.
    async fn after_tool(&mut self, call: &ToolCall, output: &mut ToolOutput,
        ctx: &PluginCtx) {}

    /// Every run event, in order.
    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {}

    /// Offered the transcript at each turn boundary, and again on a
    /// context overflow. Returning a rewrite replaces the working
    /// transcript (see "Context rewrites").
    async fn rewrite_context(&mut self, view: &ContextView<'_>,
        ctx: &PluginCtx) -> anyhow::Result<Option<Rewrite>> { Ok(None) }

    /// Called when the model has answered with no tool calls and the run
    /// would stop. `Continue(text)` adds `text` as a user message and
    /// runs another turn, at most `Limits::max_continuations` times per
    /// run (default 3).
    async fn before_stop(&mut self, message: &AssistantMessage,
        ctx: &PluginCtx) -> anyhow::Result<StopDecision> {
        Ok(StopDecision::Stop)
    }

    /// Called once the run has ended and been stored, before its outcome
    /// is returned.
    async fn finish(&mut self, run: &FinishedRun<'_>, ctx: &PluginCtx) {}
}

pub enum StopDecision {
    Stop,
    Continue(String),
}

pub struct FinishedRun<'a> {
    pub transcript: &'a [Message],
    pub stop: &'a StopReason,
    pub usage: &'a Usage,
    pub text: &'a str,
}
```

### `PluginCtx`: what a plugin can reach

```rust
pub struct PluginCtx {
    pub run: RunId,
    pub parent: Option<RunId>,
    pub agent: Arc<str>,
    /// The run's token. Plugin model calls must observe it.
    pub cancel: CancellationToken,
    /// The agent's provider, for side requests such as distilling notes.
    /// Each request opens its own session and lane.
    pub llm: Arc<dyn Llm>,
    // private: usage sink, store handle, event sender, plugin name
}

impl PluginCtx {
    /// The plugin's name.
    pub fn plugin(&self) -> &str;
    /// The run's clock, for stamping messages a plugin makes.
    pub fn now(&self) -> Timestamp;
    /// The run's retry policy, for a plugin's own model requests.
    pub fn retry_policy(&self) -> RetryPolicy;
    /// Asks the model once outside the run's conversation: its own
    /// session and lane, the run's retry policy and cancellation, every
    /// attempt's usage charged to the run. Compaction's summaries go
    /// through it.
    pub async fn ask(&self, settings: Settings, input: &[Message])
        -> anyhow::Result<AssistantMessage>;
    /// Adds usage (cost included) to the run's total, which limits
    /// check.
    pub fn charge(&self, usage: &Usage);
    /// Stores a record for this plugin in the run's transcript. The
    /// model never sees it. Forks and resumed runs get it back in
    /// `RunPlan::records`.
    pub async fn record(&self, body: &Value) -> Result<(), StoreError>;
    /// The plugin's records along the run's fork chain as stored now:
    /// its own since the start, and any an interface stored for it
    /// meanwhile (tau-goal's pause, extend and clear).
    pub async fn records(&self) -> Result<Vec<Value>, StoreError>;
}
```

## Context rewrites

```rust
pub struct ContextView<'a> {
    pub transcript: &'a [Message],
    /// The loop's estimate (compaction.md, "Token estimate").
    pub tokens: u64,
    /// The model's context window, if known.
    pub window: Option<u64>,
    pub trigger: Trigger,
    pub turn: u32,
}

pub enum Trigger {
    /// After a turn, before the next request.
    TurnEnd,
    /// The last request failed with `context_length_exceeded`.
    Overflow,
}

pub struct Rewrite {
    /// The new working transcript.
    pub messages: Vec<Message>,
    /// Plugin state to store with the rewrite: a summary, a ledger.
    pub details: Value,
}
```

- **At a turn boundary,** plugins are asked in order. The first one to
  return a rewrite wins that boundary, and the others are not asked
  again until the next one. Each plugin decides its own threshold,
  cooldown and cost.
- **On overflow,** plugins are asked in order, and the first rewrite
  wins, as between turns. The request is then retried once. Plugins are
  offered the context in the order they were added, so add cheap ones
  (pruning) before compaction; one that cannot free enough should
  decline and leave the overflow to the summary. The loop does not re-estimate after a rewrite: the
  estimate anchors on the last reported usage, which a kept message can
  still carry, so "fits the window" would not be reliable.
- **The loop checks every rewrite.** Every kept tool result must have
  its call, every kept call must have its result, and the last message
  must stay (it is the one the next request answers). A rewrite that
  fails the check is dropped and reported as an event, and the run goes
  on with the old transcript.
- **Storage.** A rewrite is stored in one write, as a `context` entry
  `{ plugin, details }` followed by the new transcript's messages.
  Loading a transcript starts at the latest `context` entry.
  Compaction's rewrite is a `context` entry from the `compaction` plugin
  whose first message is the summary.
- **Resuming.** A fork gets the details of the latest rewrite it
  inherits from `RunPlan::last_rewrite`, when its plugin made it.
  Compaction resumes its summary and file lists from there.
- **Events.** `ContextRewritten { run, plugin, tokens_before,
tokens_after }` marks each rewrite.

## Where each seam sits in the loop

```
start run
  ├─ Plugin::start (each, in order)    ← RunPlan: input, context, settings
  ├─ open session with the plan's settings, store the input
  └─ loop
       ├─ respond                        (overflow → rewrite_context(Overflow), retry once)
       ├─ tool calls: before_tool → run → after_tool
       ├─ store the turn
       ├─ no calls? before_stop (each) → Continue(text) adds a user message
       ├─ limits / cancel / steering
       └─ rewrite_context(TurnEnd)       → store, next request goes in full
  ├─ store the outcome
  └─ PluginRun::finish (each)
```

`on_event` sees every event throughout, as `RunHook::on_event` does
today.

## The shared Jev client: `tau-jev`

Three of the four plugins ask Jev (TypeSafe's System One model) for
decisions, so the client is its own small crate, not code copied three
times. It is built: `crates/plugins/jev`. Plugins take an `impl Jev`
(the trait), so tests answer with `tau_jev::fake::FakeJev` and
production uses the HTTP client, `tau_jev::TypeSafe`.

- **API.** One call: `POST https://api.typesafe.ai/v1/systemone` with
  `{ model, state, questions }`. The answers come back under the
  question ids, with `usage { input_tokens, output_tokens }`.
- **Types.** One Rust type per question kind:
  - `Choice`: pick one option, with a probability for each and a
    confidence;
  - `Score`: ordered levels (at most 10), with probabilities and a
    confidence;
  - `Noul`: the probability that a yes/no statement is true.
    A request is a builder over a shared state, so several questions go
    in one round trip, as TypeSafe's docs recommend.
- **Answers are checked when read** (`Response::noul`, `choice`,
  `score`): a missing answer, one of the wrong kind, or a probability
  outside `[0, 1]` is an error, never a default.
- **Limits.** The client does not estimate tokens: TypeSafe's limits
  (64k per request, 32k for the state plus the longest question) are
  the plugin's to respect, as fast compaction does with its own
  budgets. Text only.
- **Errors.** Transport errors leave the URL out, and error statuses
  drop the body, which can echo the request.
- **Retries.** 429, 503 and 529 are retried, honoring `retry-after`,
  under tau-ai's `RetryPolicy`.
- **Cost.** Input tokens only; output is free. Jev 1.13 costs $0.042
  per million input tokens. The client returns `Usage` with the cost
  filled in, which a plugin passes to `ctx.charge`.
- **Key.** Given explicitly, or read with `from_env()` from
  `TYPESAFE_API_KEY`, as `OpenAi` does. It never shows in `Debug`.
- **TLS.** rustls with `ring` and Mozilla's roots, as for OpenAI: no
  system certificate store.
- **Confidence.** Every answer exposes `confidence` (Choice and Score)
  or its probability (Noul). Plugins take thresholds as settings and
  have a low-confidence fallback. They never act on an unsure answer
  as if it were sure.

## The first four plugins

Token estimates and overflow detection, which the loop uses for
`ContextView` and plugins use to decide when to rewrite, are in
`tau_agent::context`.

### `tau-reasoning`: reasoning effort per job

Built: `crates/plugins/reasoning`. Runs whose effort is "auto" get
it in tau-ui when a TypeSafe key is saved; its choice (every level's
probability, the confidence and the threshold) is reported and
recorded, and shows as the run's reasoning note and plan.

- **Seam:** `start` only.
- **How:**
  1. Put the input, and the start of the instructions, into the state.
  2. Ask one `Score` question whose levels describe the work each
     effort suits. The levels are the efforts the run's model takes
     (`tau_ai::model::efforts`), so Jev never picks one the API would
     reject; a model that does not reason is not scored.
  3. If the answer is confident, set `plan.reasoning` to that level.
     Otherwise keep the agent's own setting, or a configured floor.
- **Cost:** one Jev round trip (about 180 ms median) before the session
  opens. The warm-up, if on, comes after it, so it warms the chosen
  effort.
- **Not in scope:** changing effort mid-run. That would break the
  chain every time it changed. A workflow that wants more effort for a
  later phase starts another run, and that run is scored again.
- **Sub-agents** are scored too, when their agent has the plugin. A
  cheap sub-task gets a cheap effort without the caller saying so.

### `tau-memory`: a zettelkasten on docbert

- **Seams:** tools, `start`, `finish`.
- **Tools:**
  - `memory_write { title, body, links }` creates or updates an atomic
    note;
  - `memory_search { query }` does hybrid search and returns ids,
    titles and snippets;
  - `memory_read { id }` returns a note with its links and backlinks;
  - `memory_link { from, to, why }` links two notes.
- **start:** searches for the input, and puts the top notes (ids,
  titles, one line each) into `plan.context`. The model then reads the
  ones it needs with `memory_read`.
- **finish:** optional distillation, off by default. It asks the
  agent's model (`ctx.llm`) for new notes worth keeping, and writes
  them.
- **Storage:** a directory of Markdown notes, one file each, with front
  matter holding the id and links. The notes are indexed by
  `docbert-core` as one collection.
  - Links and backlinks live in the plugin: docbert has no link graph.
  - `docbert-core` is synchronous and loads a ColBERT model on first
    use. `Memory::open` loads it once, and every call runs through
    `spawn_blocking` behind a mutex on the model manager.
- **Needed upstream in docbert:** a one-call "upsert this note"
  (Tantivy, chunks, embeddings, PLAID update) and "delete this note".
  Today a caller has to copy that sequence out of `docbert-web`'s and
  the CLI's private functions, and web ingest skips the PLAID update.
- **Scope:** a `Memory` value names one notes directory. Sharing it
  across agents shares memory; separate values keep it apart.

### `tau-constitution`: rules checked on specific calls

Built: `crates/plugins/constitution`. Its reference is
[constitution.md](constitution.md).

- **Seams:** `before_tool`, optionally `before_stop`.
- **Constitution:** a list of rules, each with an id, its text, and
  where it applies:
  - named tools and argument fields, such as `edit.newText`,
    `write.content` or `bash.command`;
  - or the final answer.
- **How:**
  1. For a call a rule applies to, put the relevant argument fields
     into the state. Only those: TypeSafe notes that irrelevant detail
     costs accuracy.
  2. Ask one `Noul` per applicable rule, all in one request.
  3. A rule whose violation probability passes its threshold blocks
     the call. The reason names the rule and quotes it, so the model
     sees why and fixes the call.
  4. A result between the thresholds is allowed and reported as an
     event, for review.
- **Final answer:** `before_stop` checks it the same way. A violation
  returns `Continue` with the rule. A cap on continuations stops a run
  from looping.
- **Risk:** the state is untrusted. File content and tool output can
  try to steer Jev, which its docs warn about. Rules for destructive
  calls get higher thresholds, and the checked fields are what the
  model wrote, not what tools returned.

### `tau-fast-compaction`: prune stale tool history

Built: `crates/plugins/fast-compaction`. Its reference is
[fast-compaction.md](fast-compaction.md).

- **Seams:** `rewrite_context` (both triggers), and `start` to restore
  its ledger.
- **How** (after `joelhooks/pi-fast-jev-compaction`):
  1. Past a share of the window (default 60%), and outside a token
     cooldown, build a state of the history: tool names, inputs,
     ok/error and sizes. Never the tool outputs themselves.
  2. Ask two `Noul`s per unpinned call: does knowing the call still
     matter, and does its full output still need to be verbatim.
     Split the questions into batches under the request limit, and send
     the batches concurrently.
  3. Decide each call: `keep`, `drop_result` (keep the head of the
     output and a note to re-run the tool) or `drop_call` (remove the
     call and its result).
  4. Decisions only escalate. They are kept as a ledger keyed by tool
     call id.
- **Output:** a `Rewrite` with the pruned transcript and the ledger in
  `details`. If the saving is under a minimum ratio, it returns `None`,
  and the next plugin (summarizing compaction) can take the boundary.
- **Why it fits tau better than pi:** pi re-applies the ledger to a
  per-turn view. Here the rewrite is stored once, so the next request
  is one full resend and then deltas again. The cooldown keeps full
  resends rare.
- **Forks** get the ledger back through `RunPlan::last_rewrite`: the
  latest `context` entry, when fast compaction made it.

### `tau-goal`: keep going until a goal holds

Built: `crates/plugins/goal`. Its reference is [goal.md](goal.md).

- **Seams:** `start` (a `/goal` input sets the goal), `after_tool` (the
  latest results, as evidence), `on_event` (what the turns cost), and
  `before_stop` (the check).
- **How:** one `Noul` per stop: does the goal hold, judged from the
  model's last answer and its recent tool results. Not yet sends the
  model back with the goal, within the goal's continuations and budget.
- **State:** records only. The goal belongs to the conversation, so a
  resumed run keeps it; an interface pauses, extends or clears it by
  storing a record, which the plugin reads at its next check.

## What changed in tau-agent

1. `Plugin`, `PluginRun`, `PluginCtx`, `RunPlan`, `ContextView`,
   `Rewrite` and `StopDecision` in a new `tau_agent::plugin` module.
   `RunHook` becomes a thin adapter onto it.
2. `Agent` builds each run's settings from its `RunPlan`, not from the
   agent directly.
3. Compaction moves behind `rewrite_context`, into its own crate,
   `tau-compaction` (`crates/plugins/compaction`); an agent adds it with
   `Agent::plugin` ([0006](../decisions/0006-plugin-crates.md)).
4. `tau-store`:
   - a `context` entry kind, which replaces `compaction`;
   - a `plugin` entry kind, for records, which the transcript skips.
5. The loop gains the new call sites:
   - `start` before the session opens;
   - `before_stop`, when a response has no tool calls;
   - `finish`, after the run is stored;
   - `rewrite_context`, at both triggers.
6. `RunEvent::ContextRewritten` marks a rewrite, and a
   `PluginError { plugin, message }` event reports a plugin failure
   that did not end the run.
7. Usage from `ctx.charge` flows into the same total as sub-agent
   usage.

## Open questions

- Should `finish` be awaited before the outcome returns, or run in the
  background? Awaiting is predictable but adds, for example, memory
  distillation to the latency of every run.
- Should plugins be able to add tools per run (from `start`), or only
  per agent? Per-run tools would cost nothing against the delta rule,
  since tools are fixed within a run. But they make an agent's surface
  harder to see.
