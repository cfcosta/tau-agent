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
  - settings change before the run opens its session. The one
    exception is the reasoning effort, which `before_request` may
    change before any turn. No model keeps its cache across such a
    change, so the next request goes in full and uncached;
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

    /// Tools resolved by name when a tool calls one (see "Nested
    /// calls"). Read once, by `Agent::plugin`.
    fn tool_source(&self) -> Option<Arc<dyn ToolSource>> {
        None
    }

    /// Prepares one run and returns the plugin's state for it. Runs
    /// before the run opens its session, in registration order, so a
    /// later plugin sees what an earlier one set. An error fails the run
    /// with `AgentError::Plugin`.
    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError>;
}
```

`Agent::plugin(p)` adds `p.tools()` to the agent, keeps
`p.tool_source()`, and keeps `p`. It is the only way to extend an
agent.

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
    //   inherits, when this plugin made it;
    // tools(): the run's tools so far (see "Nested calls").
}

impl RunPlan {
    /// Adds a tool for this run only (see "Nested calls").
    pub fn add_tool(&mut self, tool: Arc<dyn AgentTool>);
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
    /// May change the arguments (validated again) or block the call.
    /// An error blocks the call too. The first block wins.
    async fn before_tool(&mut self, call: &mut ToolCall, ctx: &PluginCtx)
        -> Result<Decision, PluginError> { Ok(Decision::Allow) }

    /// May change the output. The view holds the call (`view.call`),
    /// whether it failed, the transcript before its turn and the
    /// assistant message that made it. An error is reported as a
    /// `PluginError`.
    async fn after_tool_result(&mut self, view: &ToolResultView<'_>,
        output: &mut ToolOutput, ctx: &PluginCtx) -> Result<(), PluginError> { Ok(()) }

    /// Every run event, in order.
    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {}

    /// Called before each turn's request (not before its retries), with
    /// the transcript about to be sent, the model and the current
    /// effort. Returning an effort sets it for this request and the ones
    /// after it. The first plugin that picks one wins.
    async fn before_request(&mut self, view: &RequestView<'_>,
        ctx: &PluginCtx) -> Result<Option<ReasoningEffort>, PluginError> {
        Ok(None)
    }

    /// Offered the transcript at each turn boundary, and again on a
    /// context overflow. Returning a rewrite replaces the working
    /// transcript (see "Context rewrites").
    async fn rewrite_context(&mut self, view: &ContextView<'_>,
        ctx: &PluginCtx) -> Result<Option<Rewrite>, PluginError> { Ok(None) }

    /// Called on every plugin once a rewrite replaced the transcript,
    /// with the transcript it replaced: the last chance to keep what it
    /// dropped (tau-memory saves notes here).
    async fn rewritten(&mut self, replaced: &[Message], rewrite: &Rewrite,
        ctx: &PluginCtx) -> Result<(), PluginError> { Ok(()) }

    /// Called when the model has answered with no tool calls and the run
    /// would stop. `Continue(text)` adds `text` as a user message and
    /// runs another turn, at most `Limits::max_continuations` times per
    /// run (default 3).
    async fn before_stop(&mut self, message: &AssistantMessage,
        ctx: &PluginCtx) -> Result<StopDecision, PluginError> {
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
        -> Result<AssistantMessage, AskError>;
    /// Adds usage (cost included) to the run's total, which limits
    /// check, and to this plugin's own cost (`Store::plugin_costs`).
    /// The run emits it as `RunEvent::PluginCharged` before its next
    /// event.
    pub fn charge(&self, usage: &Usage);
    /// Reports `body` and records it with the run, in one call: what
    /// an interface shows of a plugin, live and from history alike
    /// (ADR 0017). Plugins use it for everything they report.
    pub async fn publish(&self, body: &Value) -> Result<(), StoreError>;
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
    /// Before the first request of a run that starts on a transcript
    /// it inherited or goes on with: on another model, it may not fit.
    Start,
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
  Compaction's rewrite is a `context` entry from the `tau-compaction` plugin
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
  ├─ inherited or resumed transcript? rewrite_context(Start)
  └─ loop
       ├─ before_request (each)          → the turn's effort
       ├─ respond                        (overflow → rewrite_context(Overflow), retry once)
       ├─ tool calls: before_tool → run → after_tool_result
       ├─ store the turn
       ├─ no calls? before_stop (each) → Continue(text) adds a user message
       ├─ limits / cancel / steering
       └─ rewrite_context(TurnEnd)       → store, rewritten (each), next request goes in full
  ├─ store the outcome
  └─ PluginRun::finish (each)
```

`on_event` sees every event throughout.

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
  `TYPESAFE_API_KEY`. It never shows in `Debug`.
- **TLS.** rustls with `ring` and Mozilla's roots, as for OpenAI: no
  system certificate store.
- **Confidence.** Every answer exposes `confidence` (Choice and Score)
  or its probability (Noul). Plugins take thresholds as settings and
  have a low-confidence fallback. They never act on an unsure answer
  as if it were sure.

## A plugin's UI

`tau-agent`'s `Plugin` has no UI. In this repository, each crate in
`crates/plugins` also exports a `UiPlugin` (`tau-ui-plugin`), which
builds its agent plugin for a run, folds what it publishes
(`PluginCtx::publish`) into its state for the run, live and from
history alike, and adds its pages and contributions to the interface
at extension points. `tau-ui` registers them in `plugins.rs` and builds
a run's plugins only through that registry. See
[ADR 0017](../decisions/0017-plugins-bring-their-ui.md), and its "As
built" section for the interface as it is.

## The first four plugins

Token estimates and overflow detection, which the loop uses for
`ContextView` and plugins use to decide when to rewrite, are in
`tau_agent::context`.

### `tau-reasoning`: reasoning effort per job

Built: `crates/plugins/reasoning`. Runs whose effort is "auto" get
it in tau-ui when a TypeSafe key is saved; its choice (every level's
probability, the confidence and the threshold) is reported and
recorded, and shows as the run's reasoning note and plan. The Models
screen's "Reasoning on auto" sets `redecide` ("Decide again between
steps") and the threshold (0.5 to 0.9, 0.7 by default) for runs started
after; they are saved with the model settings.

- **Seams:** `start`, and `before_request` when `Reasoning::redecide`
  is on. `finish` leaves the next message its context.
- **How, as a message comes in:**
  1. Put the task, clipped at both ends (600 characters from the
     start, 200 from the end), and the start of the instructions into
     the state. A message of 120 characters or fewer also brings the
     previous message's task and the agent's last words
     (`previous_task`, `last_proposal`, from the record `finish` left):
     "yes, do it" is only as simple as what it agrees to.
  2. Ask one `Score` question whose levels describe the work each
     effort suits. The levels are the efforts the run's model takes
     (`tau_ai::model::efforts`), so Jev never picks one the API would
     reject; a model that does not reason is not scored. The question
     asks for the minimum sufficient depth, says that a short message
     alone is not evidence the work is simple, and that the state is
     evidence, not instructions.
  3. If the answer is confident, set `plan.reasoning` to that level.
     Otherwise, or if the request fails, go on at the effort the run's
     last message ran at (from the plugin's records), if the model takes
     it, else at the model's default. The record says what the message
     runs at (`runs_at`); tau-ui shows a note only when that changes.
- **How, between turns** (only with `redecide`, off by default): the
  first question
  also asks, as a `Choice`, how long the effort holds:
  - `one_call`: the next request only;
  - `tool_chain`: while the tool calls succeed;
  - `user_turn`: until the user writes again.

  A failed tool call, a context rewrite or a user message steered in
  ends any lease. Before a request whose lease has ended, Jev is asked
  both questions again, with the step (`tool_step`, or `user_turn` for
  a steered message), the effort now, the end of what the agent last
  said, and the tool results it is about to read: how many, how many
  failed, and up to three excerpts, failed ones first. A confident
  answer sets the effort for that request on; an unsure or failed one
  leaves it. Each choice is reported and recorded with its `turn` and
  `lease`, and tau-ui shows it before the reply it chose for, with how
  long the effort holds. A request Jev fails is reported and recorded
  as well (`kind: "error"`, with its `turn` between turns), so a stored
  run shows it too.

- **Cost:** one Jev round trip (about 180 ms median) before the session
  opens, and with `redecide` one per ended lease. The warm-up, if on,
  comes after the first, so it warms the chosen effort.
- **Why `redecide` is off:** no model keeps its cache across a change
  of effort (measured on Luna, Sol, Astra and Terra; see
  [openai-websocket.md](openai-websocket.md)), so every change makes
  the next request a full, uncached resend. It can still pay when a
  long tool chain drops from a high effort to a low one; turn it on
  only where that trade is worth it. A workflow that wants more effort
  for a later phase can instead start another run, which is scored
  again.
- **Sub-agents** are scored too, when their agent has the plugin. A
  cheap sub-task gets a cheap effort without the caller saying so.

#### Replaying stored runs

`tau_reasoning::replay` walks a stored run's timeline request by request
through the same policy: a user message after a final answer is scored
as `start` would score it, and a later request only when the simulated
lease has ended. Each `Decision` says what the stored request went out
at (from the run's records), what Jev answered, and what the policy
would send. The example runs it on the latest runs of a store, each on
its own model, with the real Jev:

```sh
cargo run -p tau-reasoning --example replay -- ~/.local/share/tau/runs.db 20
```

It reports decisions, not savings: the reasoning tokens a request
would have spent at another effort were never spent, so no stored
number says what it would have cost. The store keeps no agent
instructions, so the replay goes without them.

### `tau-memory`: a zettelkasten on docbert

Research and the reasons behind these choices:
[research/memory.md](../research/memory.md). Decided 2026-09-29.

- **Seams:** tools, `start`, and `rewritten`, for a memory-only turn
  over what compaction dropped.
- **Notes:** one idea per Markdown file at `<type>/<slug>.md`, the title
  stating the claim, the body in full prose with exact versions, flags,
  paths and error strings.
  - Front matter: `id`, `title`, `description` (one line), `type`,
    `tags`, `created`, `updated`, `valid_from`, `valid_to`, `source`
    (run, turn, commit, files; said by the user, done by the agent, or
    inferred) and `links`.
  - Types, a closed set: `fact`, `convention`, `decision`, `gotcha`
    (symptom, cause, fix), `procedure`, `case` (task, approach,
    outcome), `preference`, `index`. Task progress and TODOs are not
    memory.
  - Links, a closed set: `relates` (a bare `[[x]]`), `refines`,
    `supersedes`, `contradicts`, `derived_from`, `about` (a file, crate
    or symbol). Backlinks live in the index; a link to a note not
    written yet is kept and resolves when it is.
  - A changed fact is a new note that `supersedes` the old one; the old
    one keeps its text and gets `valid_to`. Nothing is deleted by the
    model.
- **Tools:**
  - `memory_write { type, title, body, links, supersedes? }` searches
    first and returns the nearest notes with the result, so the agent
    updates, supersedes or links instead of duplicating;
  - `memory_search { query }` returns ids, titles, types, descriptions
    and snippets from docbert, plus one hop along links, in one budget;
    superseded notes rank lower and are labelled, never hidden;
  - `memory_read { id }` returns a note with its links and backlinks;
  - `memory_link { from, to, type, why }` links two notes.
  - Notes are plain files too, so the agent can grep them.
- **The index note:** written by the agent, like Claude Code's
  `MEMORY.md`, within about 2k tokens. A write past the budget is
  refused with a message asking the agent to rewrite it; nothing is cut
  silently.
- **start:** puts the index note into `plan.context`, frozen for the
  run, then the top few docbert hits for the input (about three), both
  fenced as untrusted data.
- **At compaction:** in `rewritten`, one model request (`ctx.ask`) over
  the replaced transcript with only the memory tools, since compaction
  is where details are lost.
- **After a run:** a background consolidation pass (signal gate,
  faithful to the transcript, search before write, one reviewable
  commit) exists but is **off** until the evaluation shows it helps.
- **Staleness:** a note `about` a file is marked "may be stale" when a
  later turn's commit touches that file; the agent sees the mark and
  re-checks.
  - The app hears of each turn's commit through `RunWorkspace::on_commit`,
    with the paths it changed, so a change made through bash counts as
    much as one through the edit tools.
  - Only notes last written before the turn began are marked: a commit
    can hold edits made before a note about that file was written.
- **Storage:** tau's data directory, per repository, versioned so every
  change is a diff that can be reviewed or reverted; nothing lands in
  the project's history. Notes are the truth; the search index is
  derived and rebuilt from them on open.
  - The index is ColBERT MaxSim alone. The `docbert` feature encodes
    with docbert's model through `docbert-pylate`, loaded on first use;
    without a model (as in tests) memory searches with BM25.
  - It was BM25 fused with ColBERT by reciprocal rank fusion, as docbert
    fuses them. The retrieval evaluation showed the fusion doing worse:
    - ColBERT alone found verbatim queries as well as BM25 did, and found
      paraphrases far more often (recall at 5 of 0.83 against the
      fusion's 0.54, and 0.58 against 0.29 with 16 near-duplicates per
      answer).
    - The fusion lets BM25 rank near-duplicates that share a query's
      words above the answer.
  - Embeddings are cached on disk by model and text, so reopening a
    scope encodes only notes that changed.
  - Every note is scored exhaustively: a scope is small, and MaxSim over
    all of it is exact. PLAID is out for now.
- **Safety:** secrets are redacted before a note is written, writes are
  scanned for prompt injection, and memory never stands in for rules
  (those belong in `AGENTS.md` or the constitution).
- **Evaluation first:** retrieval recall at 5 and 10 by evidence
  distance; coding tasks that need an earlier run, before and after the
  fact changed; recall as near-duplicates pile up, for BM25, ColBERT and
  the hybrid (does late interaction escape the interference "The Price
  of Meaning" proves?); calls, tokens and latency. Baselines: no memory,
  one `MEMORY.md`, docbert over raw transcripts.
  - Retrieval and interference run today: `cargo run --release -p
tau-memory --features docbert --bin tau-memory-eval` (`--keywords`
    for BM25 alone, `--json PATH` for the rows). The corpus is
    `crates/plugins/memory/eval/harbor.toml`, synthetic facts about a
    made-up service. Every level holds the same number of notes, so only
    the near-duplicates per answer change.
  - The end-to-end tasks run with `cargo run --release -p tau-memory-e2e`
    (`--features docbert` to search as the app does; BM25 without it).
    They make real model calls, which cost money: `--budget-usd` stops
    the run once that much is spent, and `--scenario`, `--arm`,
    `--variant` and `--trials N` pick what runs. `--help` lists the rest.
    - Access is a saved ChatGPT sign-in with plan usage: `--chatgpt ID`,
      or tau's active account. The model defaults to tau-ui's. Without
      a sign-in, nothing runs.
    - Five scenarios in `crates/evals/memory-e2e`, each a small bash
      repository made in a temporary directory. The first run finds a
      fact by doing a task (the suite needs an environment variable,
      code is generated, settings keys take a prefix, a release touches
      three files, commands are registered in a list); the second run
      needs it for another task, and a check on the repository decides
      success.
    - In the changed variant a commit between the runs changes the fact,
      and memory marks the notes about the changed files stale, as the
      app does. A second run that uses the old fact is counted.
    - Arms: `none`; `memory_md` (the agent keeps `MEMORY.md`, and the
      second run starts with it); `transcripts` (the best chunks of the
      first run's transcript for the second task); `memory`; and
      `memory_consolidate`. The memory arms share a fresh scope between
      a trial's two runs.
    - Per run: success, tool calls, input, output and cached tokens,
      cost, wall time; per trial, whether the old fact was used and
      whether the second run read memory. It prints the means per arm
      and variant; `--json PATH` writes every trial.
- **Scope:** per repository, plus a user scope for preferences across
  projects; a fact lives in exactly one.
  - In the app, a repository's notes are in `memory/` in tau's directory
    for it, beside its constitution, and the user's in `memory/` in
    tau's data directory. Each scope opens once and every run shares it.
  - Plugin context sits in the run's first message as text blocks before
    the input; the app shows only the last block as what the user
    wrote.

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

- **Seams:** `rewrite_context` (both triggers), `start` to restore
  its ledger, and `after_tool_result` to prune a large `bash` output
  before the model first sees it.
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

- **Seams:** `start` (a `/goal` input sets the goal),
  `after_tool_result` (the latest results, as evidence), `on_event`
  (what the turns cost), and `before_stop` (the check).
- **How:** one `Noul` per stop: does the goal hold, judged from the
  model's last answer and its recent tool results. Not yet sends the
  model back with the goal, within the goal's continuations and budget.
- **State:** records only. The goal belongs to the conversation, so a
  resumed run keeps it; an interface pauses, extends or clears it by
  storing a record, which the plugin reads at its next check.

## What changed in tau-agent

1. `Plugin`, `PluginRun`, `PluginCtx`, `RunPlan`, `ContextView`,
   `Rewrite` and `StopDecision` in a new `tau_agent::plugin` module,
   with `ToolCall` and `Decision`. `RunHook` and `Agent::hook` are
   gone: plugins are the only extension.
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
   usage, and is also stored per plugin (`plugin_costs`). The run emits
   each charge as `RunEvent::PluginCharged { plugin, usage }`, after
   `RunStart` for a charge made in `start`. A charge made in `finish`
   comes after `RunEnd`, so it is only stored. Interfaces show what
   plugins cost from these, not from the costs plugins put in their
   reports.
8. Tools call tools: `ToolCtx::call` and `catalog`, `Exposure`,
   `ToolOutput::structured`, `ToolSource`, `RunPlan::add_tool` and
   `ToolCtx::plugin` (see "Nested calls"). Tool events and the plugins'
   `ToolCall` gain `parent`.

## Nested calls

Built in `tau_agent::tool`, `tau_agent::plugin` and the loop. Decided
in [0018](../decisions/0018-codemode-and-mcp.md), for `tau-codemode`
and `tau-mcp`.

```rust
pub enum Exposure {
    /// Declared to the model, and callable from tools. The default.
    Direct,
    /// Callable from tools only: never declared, so it costs nothing
    /// against the delta rule.
    Nested,
    /// Declared to the model, never callable from tools (`codemode`).
    ModelOnly,
}

pub trait AgentTool {
    // ...
    fn exposure(&self) -> Exposure { Exposure::Direct }
    /// The JSON schema of `ToolOutput::structured`, if the tool fills it.
    fn output_schema(&self) -> Option<&Value> { None }
}

pub struct ToolOutput {
    pub content: Vec<InputBlock>,
    pub details: Option<Value>,
    /// What a calling tool gets instead of the text; the model never
    /// sees it, and the transcript does not keep it.
    pub structured: Option<Value>,
}

/// Tools that come and go while runs go on, resolved by name at call
/// time: an MCP server's.
#[async_trait]
pub trait ToolSource: Send + Sync + 'static {
    fn tools(&self) -> Vec<Arc<dyn AgentTool>>;
    /// Empty by default.
    fn namespaces(&self) -> Vec<Namespace> { Vec::new() }
    /// Waits until the namespaces are ready, or all of them for `None`,
    /// or until `cancel`. Returns at once by default.
    async fn ready(&self, namespaces: Option<&[String]>, cancel: &CancellationToken) {}
}

/// A group of tools, such as one MCP server's.
pub struct Namespace {
    pub name: String,                 // `mcp__linear`
    pub description: String,
    pub instructions: Option<String>, // a server's instructions
    pub tools: Vec<String>,           // its tools' names
}

impl ToolCtx {
    /// Every tool this call could call, as the run has them now.
    pub fn catalog(&self) -> Catalog;
    /// Calls a tool through the loop, as `<this call's id>/<n>`.
    pub async fn call(&self, name: &str, args: Value)
        -> Result<ToolOutput, ToolError>;
    /// The `PluginCtx` of the plugin that added this tool, for the run.
    pub fn plugin(&self) -> Option<&PluginCtx>;
    /// This call's id: the model's call id, or `<parent>/<n>`.
    pub fn call_id(&self) -> &str;
}

impl Catalog {
    /// The run's `Direct` and `Nested` tools, in the order they were
    /// added, then each source's callable tools that no run tool's name
    /// hides (the first source to offer a name wins).
    pub fn tools(&self) -> &[Arc<dyn AgentTool>];
    pub fn get(&self, name: &str) -> Option<&Arc<dyn AgentTool>>;
    /// The sources' namespaces.
    pub fn namespaces(&self) -> &[Namespace];
    pub fn namespace(&self, name: &str) -> Option<&Namespace>;
    /// Waits on every source's `ready`. A catalog taken afterwards shows
    /// the tools they brought.
    pub async fn ready(&self, namespaces: Option<&[String]>, cancel: &CancellationToken);
}

impl RunPlan {
    /// Adds a tool for this run only: tau-mcp's direct tools.
    pub fn add_tool(&mut self, tool: Arc<dyn AgentTool>);
    /// The run's tools, the agent's and the ones added so far.
    pub fn tools(&self) -> &[Arc<dyn AgentTool>];
}

pub struct ToolCall {        // what plugins see, in `tau_agent::plugin`
    pub id: String,
    pub name: String,
    pub args: Value,
    /// For a nested call, the id of the call that made it.
    pub parent: Option<String>,
}
```

- **Who sees what.** The model is declared the run's `Direct` and
  `ModelOnly` tools, and can call only those: a call to a `Nested` tool
  from the model is unknown, as any undeclared name is. A tool can call
  the run's `Direct` and `Nested` tools and the sources' tools.
- **Sources.** `Plugin::tool_source()` returns the plugin's
  `ToolSource`, if any. Its tools are never declared, whatever their
  exposure, and are looked up by name on each call, so a script reaches
  a server that connected after the run started. They should be
  `Nested`; a `ModelOnly` one is not callable, and a tool the model
  should see goes through `add_tool`. A run tool hides a source tool of
  the same name.
- **Per-run tools.** `RunPlan::add_tool` in `Plugin::start` adds a tool
  for that run; a tool of the same name already in the plan is replaced
  in its place. The run's tools are fixed once its session opens, so
  this costs nothing against the delta rule. A tool added with an
  invalid schema fails the run with `AgentError::Schema`.
- **A plugin's tools reach its run.** `ToolCtx::plugin()` is the run's
  `PluginCtx` of the plugin that added the tool, by `Plugin::tools`,
  `add_tool` or its source; `None` for the agent's own tools.
- **Through the loop.** `ToolCtx::call` sends the call to the run's
  loop, which treats it as it treats a model's call: lookup, argument
  repair, validation, `before_tool` and `after_tool_result` for every
  plugin, `ToolStart`, `ToolUpdate` and `ToolEnd` with
  `parent: Some(<the calling call's id>)`. The loop polls nested calls
  in the same `select!` as the batch, so a plugin is still called one
  call at a time, and the nested calls' futures run alongside the
  batch's.
- **Ids.** A nested call's id is `<parent>/<n>`, `n` counting from 1 in
  the order the loop receives the calls. A nested call can make nested
  calls of its own: `call_1/2/1`.
- **Results.** The result goes back to the caller only, never into the
  transcript. A failed call is `Err(ToolError::Output(output))`, with
  the output the model would have seen and the tool's `structured`
  value, if any. `ToolResultView::message` is the turn that made the
  outer call.
- **Scheduling.** Among one caller's nested calls, a `Sequential`
  tool's call waits for the others to finish and runs alone; the
  others run at once. A `Grouped` tool's nested calls run as
  `Parallel` ones do.
- **Cancel.** A nested call's token is a child of its caller's, so the
  run's cancel reaches it. Ending the outer call cancels the nested
  calls it left running, and fails the ones still waiting to start;
  their `ToolEnd`s may come after the outer call's.
- **In tau-ui.** A nested call gets no card. Its events fold into the
  `CallData::nested` rows of the card of the model's call it came from,
  at any depth, and the card's data drops them when that call ends,
  since a stored run has no events; a tool whose calls should outlast
  it lists them in its result's details (codemode's `calls`). A fold's
  `RunCx::attach` for a nested id lands on that card, `mark` keeps the
  verdict in `CallData::nested_marks` for the call's row without
  changing the card's state, and `dropped` and `cut` answer false: a
  nested result never reaches the model's context. The Events tab lists
  nested calls as `NestedStart` and `NestedEnd`. What the workspace
  does for a call it does for a nested one too: a nested `vcs_land`
  proposes the run's landing, and a nested `delegate`'s sub-agent gets
  its task and closes when that call returns (`vcs.md`). After the
  model's call ends, they read its `details.calls`, whose rows have
  at least `name` and `status` (`ok` for a call that succeeded).
- **Not callable:** unknown names (`Tool x not found`), `ModelOnly`
  tools (`Tool x cannot be called from a tool`), and any call once the
  outer call has ended, or outside a run. Each fails with a message.

## Open questions

- Should `finish` be awaited before the outcome returns, or run in the
  background? Awaiting is predictable but adds, for example, memory
  distillation to the latency of every run.
- ~~Should plugins be able to add tools per run?~~ Yes, from `start`
  (`RunPlan::add_tool`), decided in
  [0018](../decisions/0018-codemode-and-mcp.md); see "Nested calls".
