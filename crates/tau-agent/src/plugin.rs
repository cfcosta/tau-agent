//! Plugins: extensions an agent gets from other crates
//! (`docs/reference/plugins.md`).
//!
//! A [`Plugin`] is shared by every run of an agent. It adds tools, and
//! for each run it prepares a [`RunPlan`] and returns a [`PluginRun`]
//! that holds that run's state. The loop calls a run's plugins one at a
//! time, in registration order, so a `PluginRun` needs no locks.
//!
//! The seams follow the WebSocket delta rule: a run's settings change
//! only in [`Plugin::start`], before its session opens, and the other
//! seams leave the request alone.

use std::sync::{
    Arc,
    Mutex,
    atomic::{AtomicI64, Ordering},
};

use async_trait::async_trait;
use serde_json::Value;
use tau_ai::{
    llm::Llm,
    message::{AssistantMessage, Message, Timestamp, Usage},
    responses::request::ReasoningEffort,
    retry::RetryPolicy,
};
use tau_store::{Entry, RunKind, Store, StoreError, TurnUsage};
use tokio_util::sync::CancellationToken;

use crate::{
    event::{RunEvent, StopReason},
    hook::{Decision, HookCtx, RunHook, ToolCall},
    runner::{Clock, add_usage},
    tool::{AgentTool, RunId, ToolOutput},
};

/// An extension of an agent. Added with `Agent::plugin`.
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// Names the plugin in events, errors and stored records.
    fn name(&self) -> &str;

    /// Tools the plugin adds to the agent. Read once, by `Agent::plugin`.
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        Vec::new()
    }

    /// Prepares one run and returns the plugin's state for it.
    ///
    /// Runs before the run opens its session, in registration order, so
    /// a later plugin sees what an earlier one set in `plan`. An error
    /// fails the run with `AgentError::Plugin`.
    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Box<dyn PluginRun>> {
        let _ = (plan, ctx);
        Ok(Box::new(()))
    }
}

/// A plugin's part in one run. Every method does nothing by default.
#[async_trait]
pub trait PluginRun: Send {
    /// Runs before a tool call, after its arguments were validated. May
    /// change the arguments, which are then validated again, or block
    /// the call; the reason becomes the result the model sees. An error
    /// blocks the call too. The first plugin that blocks wins.
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Decision> {
        let _ = (call, ctx);
        Ok(Decision::Allow)
    }

    /// Runs after a tool call, and may change its output.
    async fn after_tool(
        &mut self,
        call: &ToolCall,
        output: &mut ToolOutput,
        ctx: &PluginCtx,
    ) {
        let _ = (call, output, ctx);
    }

    /// Sees every run event, in order.
    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        let _ = (event, ctx);
    }

    /// Offered the transcript between turns, and when a request failed
    /// because the context overflowed. A [`Rewrite`] replaces the working
    /// transcript: the loop checks it, stores it, and sends the next
    /// request in full. The first plugin that rewrites wins; on an
    /// overflow, the request is then retried once. An error is reported
    /// as `RunEvent::PluginError` and counts as no rewrite; on an
    /// overflow that nobody rewrote, it joins the run's error.
    async fn rewrite_context(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Option<Rewrite>> {
        let _ = (view, ctx);
        Ok(None)
    }

    /// Runs when the model answered with no tool calls and the run would
    /// stop. [`StopDecision::Continue`] adds its text as a user message
    /// and runs another turn, up to `Limits::max_continuations` times per
    /// run. The first plugin that continues wins. An error is reported
    /// as `RunEvent::PluginError` and counts as `Stop`.
    async fn before_stop(
        &mut self,
        message: &AssistantMessage,
        ctx: &PluginCtx,
    ) -> anyhow::Result<StopDecision> {
        let _ = (message, ctx);
        Ok(StopDecision::Stop)
    }

    /// Runs once the run has ended and its outcome is stored, before the
    /// outcome is returned.
    async fn finish(&mut self, run: &FinishedRun<'_>, ctx: &PluginCtx) {
        let _ = (run, ctx);
    }
}

/// The part in a run of a plugin that keeps no state per run.
#[async_trait]
impl PluginRun for () {}

/// What [`PluginRun::rewrite_context`] is offered.
#[derive(Debug, Clone, Copy)]
pub struct ContextView<'a> {
    pub transcript: &'a [Message],
    /// The loop's estimate of the transcript's size
    /// (`docs/reference/compaction.md`, "Token estimate").
    pub tokens: u64,
    /// The model's context window, when the model registry knows it.
    pub window: Option<u64>,
    pub trigger: Trigger,
    /// The turn that just ended, or that overflowed.
    pub turn: u32,
}

/// Why the context is offered for a rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    /// A turn ended, and the run goes on.
    TurnEnd,
    /// The last request failed because the context was too long.
    Overflow,
}

/// A new working transcript, from [`PluginRun::rewrite_context`].
///
/// The loop rejects a rewrite, as a `PluginError`, unless it is not
/// empty, ends with the message the transcript ends with (the one the
/// next request answers), and keeps every tool call paired with its
/// result.
#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub messages: Vec<Message>,
    /// Stored with the rewrite. A fork of the run gets it back from
    /// [`RunPlan::last_rewrite`] when the rewrite is the latest.
    pub details: Value,
}

/// Whether a run may stop. See [`PluginRun::before_stop`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopDecision {
    Stop,
    /// Runs another turn, with this text as the user's message.
    Continue(String),
}

/// A run as it ended, for [`PluginRun::finish`].
#[derive(Debug, Clone, Copy)]
pub struct FinishedRun<'a> {
    pub transcript: &'a [Message],
    pub stop: &'a StopReason,
    /// The run's usage, its children's and its plugins' included.
    pub usage: &'a Usage,
    /// The text of the last assistant message.
    pub text: &'a str,
}

/// What a run starts with. Plugins may change it in [`Plugin::start`];
/// nothing changes it once the run's session is open.
#[derive(Debug, Clone)]
pub struct RunPlan {
    /// The user's input.
    pub input: String,
    /// Texts put before the input, in the run's first user message, in
    /// order. Unlike the instructions, they differ from run to run
    /// without costing the prompt cache the instructions.
    pub context: Vec<String>,
    pub instructions: Option<String>,
    pub reasoning: Option<ReasoningEffort>,
    model: String,
    kind: RunKind,
    workflow: Option<Arc<str>>,
    records: Vec<Value>,
    last_rewrite: Option<Value>,
}

impl RunPlan {
    pub(crate) fn new(
        input: String,
        instructions: Option<String>,
        reasoning: Option<ReasoningEffort>,
        model: String,
        kind: RunKind,
        workflow: Option<Arc<str>>,
    ) -> Self {
        Self {
            input,
            context: Vec::new(),
            instructions,
            reasoning,
            model,
            kind,
            workflow,
            records: Vec::new(),
            last_rewrite: None,
        }
    }

    /// The model the run asks.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Whether the run is a root run, a fork or a sub-agent.
    pub fn kind(&self) -> &RunKind {
        &self.kind
    }

    pub fn workflow(&self) -> Option<&str> {
        self.workflow.as_deref()
    }

    /// The records the plugin being started stored along the run's fork
    /// chain (see [`PluginCtx::record`]), oldest first. A root run has
    /// none; a fork gets its ancestors', to resume from.
    pub fn records(&self) -> &[Value] {
        &self.records
    }

    /// The details of the latest context rewrite the run inherits, when
    /// the plugin being started made it. A fork of a compacted run gets
    /// the compaction's record here.
    pub fn last_rewrite(&self) -> Option<&Value> {
        self.last_rewrite.as_ref()
    }

    pub(crate) fn set_records(&mut self, records: Vec<Value>) {
        self.records = records;
    }

    pub(crate) fn set_last_rewrite(&mut self, details: Option<Value>) {
        self.last_rewrite = details;
    }
}

/// Usage that plugins charged to a run.
#[derive(Debug, Default)]
pub(crate) struct Charged {
    /// Everything charged, which counts toward the run's limits.
    pub total: Usage,
    /// What the store does not have yet; the loop writes it with the
    /// next turn.
    pub unsaved: Usage,
}

/// What a plugin can reach during a run.
#[derive(Clone)]
pub struct PluginCtx {
    pub run: RunId,
    pub parent: Option<RunId>,
    pub agent: Arc<str>,
    /// The run's cancellation token. A plugin's own model calls must
    /// observe it.
    pub cancel: CancellationToken,
    /// The agent's model provider, for side requests. Each session a
    /// plugin opens has its own lane.
    pub llm: Arc<dyn Llm>,
    plugin: Arc<str>,
    store: Store,
    charged: Arc<Mutex<Charged>>,
    last_seq: Arc<AtomicI64>,
    clock: Clock,
    retry: RetryPolicy,
}

impl std::fmt::Debug for PluginCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginCtx")
            .field("run", &self.run)
            .field("plugin", &self.plugin)
            .finish_non_exhaustive()
    }
}

/// The parts of a run every plugin context shares.
#[derive(Clone)]
pub(crate) struct RunShared {
    pub run: RunId,
    pub parent: Option<RunId>,
    pub agent: Arc<str>,
    pub cancel: CancellationToken,
    pub llm: Arc<dyn Llm>,
    pub store: Store,
    pub charged: Arc<Mutex<Charged>>,
    /// The `seq` of the run's last stored entry, -1 before the first.
    pub last_seq: Arc<AtomicI64>,
    pub clock: Clock,
    pub retry: RetryPolicy,
}

impl RunShared {
    pub(crate) fn ctx(&self, plugin: &str) -> PluginCtx {
        PluginCtx {
            run: self.run.clone(),
            parent: self.parent.clone(),
            agent: self.agent.clone(),
            cancel: self.cancel.clone(),
            llm: self.llm.clone(),
            plugin: plugin.into(),
            store: self.store.clone(),
            charged: self.charged.clone(),
            last_seq: self.last_seq.clone(),
            clock: self.clock.clone(),
            retry: self.retry,
        }
    }
}

impl PluginCtx {
    /// The plugin this context belongs to.
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    /// The run's clock, which stamps its messages: a plugin that makes
    /// messages stamps them with it.
    pub fn now(&self) -> Timestamp {
        (self.clock)()
    }

    /// How the run retries failed model requests. A plugin's own
    /// requests go through it too.
    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry
    }

    /// Charges usage, cost included, to the run: it counts toward the
    /// run's limits and outcome, and is stored with the run's totals.
    pub fn charge(&self, usage: &Usage) {
        let mut charged = self.charged.lock().expect("not poisoned");
        add_usage(&mut charged.total, usage);
        add_usage(&mut charged.unsaved, usage);
    }

    /// Stores a record for this plugin with the run. The model never sees
    /// it. Forks of the run get it back in [`RunPlan::records`].
    pub async fn record(&self, body: &Value) -> Result<(), StoreError> {
        let entry = Entry::Plugin {
            plugin: self.plugin.to_string(),
            body: body.to_string(),
        };
        let last = self
            .store
            .append_turn(&self.run.0, &[entry], TurnUsage::default())
            .await?;
        self.last_seq.fetch_max(last, Ordering::SeqCst);
        Ok(())
    }
}

/// A [`RunHook`] as a plugin: every run shares the one hook.
pub(crate) struct Hooked(pub Arc<dyn RunHook>);

#[async_trait]
impl Plugin for Hooked {
    fn name(&self) -> &str {
        "hook"
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> anyhow::Result<Box<dyn PluginRun>> {
        Ok(Box::new(HookedRun(self.0.clone())))
    }
}

struct HookedRun(Arc<dyn RunHook>);

fn hook_ctx(ctx: &PluginCtx) -> HookCtx {
    HookCtx {
        run: ctx.run.clone(),
        parent: ctx.parent.clone(),
    }
}

#[async_trait]
impl PluginRun for HookedRun {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Decision> {
        self.0.before_tool(call, &hook_ctx(ctx)).await
    }

    async fn after_tool(
        &mut self,
        call: &ToolCall,
        output: &mut ToolOutput,
        ctx: &PluginCtx,
    ) {
        self.0.after_tool(call, output, &hook_ctx(ctx)).await
    }

    async fn on_event(&mut self, event: &RunEvent, _ctx: &PluginCtx) {
        self.0.on_event(event).await
    }
}
