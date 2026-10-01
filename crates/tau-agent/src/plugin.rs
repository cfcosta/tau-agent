//! Plugins: extensions an agent gets from other crates
//! (`docs/reference/plugins.md`).
//!
//! A [`Plugin`] is shared by every run of an agent. It adds tools, and
//! for each run it prepares a [`RunPlan`] and returns a [`PluginRun`]
//! that holds that run's state. The loop calls a run's plugins one at a
//! time, in registration order, so a `PluginRun` needs no locks.
//!
//! The seams follow the WebSocket delta rule: a run's settings change
//! in [`Plugin::start`], before its session opens. The one exception is
//! [`PluginRun::before_request`], which may change the reasoning effort;
//! every change costs the next request a full, uncached resend.

use std::sync::{
    Arc,
    Mutex,
    atomic::{AtomicI64, Ordering},
};

use async_trait::async_trait;
use serde_json::Value;
use tau_ai::{
    llm::{Llm, LlmError},
    message::{AssistantMessage, Message, Timestamp, Usage},
    responses::request::{ReasoningEffort, Settings},
    retry::{Class, RetryPolicy, jitter},
};
use tau_store::{Entry, RunKind, Store, StoreError, TurnUsage};
use tokio_util::sync::CancellationToken;

pub use crate::error::PluginError;
use crate::{
    event::{RunEvent, StopReason},
    runner::{
        Clock,
        respond::{self, Reading, Streamed},
    },
    tool::{AgentTool, RunId, ToolOutput, ToolSource},
    validation::ArgumentSchema,
};

/// The record `body` holds, when it reads as `R`. One that does not is
/// skipped, and said once per plugin: a record the plugin can no longer
/// read must not stop a run.
pub fn read_record<R: serde::de::DeserializeOwned>(
    plugin: &str,
    body: &Value,
) -> Option<R> {
    match serde_json::from_value(body.clone()) {
        Ok(record) => Some(record),
        Err(error) => {
            static LOGGED: Mutex<std::collections::BTreeSet<String>> =
                Mutex::new(std::collections::BTreeSet::new());
            if LOGGED
                .lock()
                .expect("not poisoned")
                .insert(plugin.to_owned())
            {
                eprintln!("{plugin}: skipped a record it cannot read: {error}");
            }
            None
        }
    }
}

/// The records among `bodies` that read as `R`, in order
/// ([`read_record`]).
pub fn read_records<R: serde::de::DeserializeOwned>(
    plugin: &str,
    bodies: &[Value],
) -> Vec<R> {
    bodies
        .iter()
        .filter_map(|body| read_record(plugin, body))
        .collect()
}

/// A tool call as plugins see it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
    /// For a nested call (`ToolCtx::call`), the id of the call that made
    /// it. Nested calls never reach the transcript, so a plugin that
    /// keeps a ledger of the transcript's calls skips them.
    pub parent: Option<String>,
}

/// Whether a tool call may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Refuses the call; the reason becomes the error result the model
    /// sees.
    Block(String),
}

/// An extension of an agent. Added with `Agent::plugin`.
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// Names the plugin in events, errors and stored records.
    fn name(&self) -> &str;

    /// Tools the plugin adds to the agent. Read once, by `Agent::plugin`.
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        Vec::new()
    }

    /// Tools resolved by name when a tool calls them, which may come and
    /// go while runs go on: an MCP server's. They are never declared to
    /// the model. Read once, by `Agent::plugin`.
    fn tool_source(&self) -> Option<Arc<dyn ToolSource>> {
        None
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
    ) -> Result<Box<dyn PluginRun>, PluginError> {
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
    ) -> Result<Decision, PluginError> {
        let _ = (call, ctx);
        Ok(Decision::Allow)
    }

    /// Runs after a tool call, with whether it failed and what the model
    /// had seen when it made the call, and may change its output. An
    /// error is reported as `RunEvent::PluginError`, and the output goes
    /// on as the plugin left it.
    async fn after_tool_result(
        &mut self,
        view: &ToolResultView<'_>,
        output: &mut ToolOutput,
        ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        let _ = (view, output, ctx);
        Ok(())
    }

    /// Sees every run event, in order.
    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        let _ = (event, ctx);
    }

    /// Runs before each turn's request, retries and overflow retries
    /// excepted, and may set the reasoning effort for it and the requests
    /// after it. The first plugin that picks an effort wins. An error is
    /// reported as `RunEvent::PluginError` and picks nothing.
    async fn before_request(
        &mut self,
        view: &RequestView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<ReasoningEffort>, PluginError> {
        let _ = (view, ctx);
        Ok(None)
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
    ) -> Result<Option<Rewrite>, PluginError> {
        let _ = (view, ctx);
        Ok(None)
    }

    /// Runs, for every plugin, once a rewrite from any plugin replaced the
    /// working transcript, with the transcript it replaced: the last
    /// chance to keep what the rewrite dropped, before the next request.
    /// An error is reported as `RunEvent::PluginError`.
    async fn rewritten(
        &mut self,
        replaced: &[Message],
        rewrite: &Rewrite,
        ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        let _ = (replaced, rewrite, ctx);
        Ok(())
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
    ) -> Result<StopDecision, PluginError> {
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

/// What [`PluginRun::after_tool_result`] is offered.
#[derive(Debug, Clone, Copy)]
pub struct ToolResultView<'a> {
    pub call: &'a ToolCall,
    /// Whether the call failed; its output is then the error.
    pub is_error: bool,
    /// The working transcript before the turn that made the call.
    pub transcript: &'a [Message],
    /// The assistant message that made the call.
    pub message: &'a AssistantMessage,
}

/// What [`PluginRun::before_request`] is offered.
#[derive(Debug, Clone, Copy)]
pub struct RequestView<'a> {
    /// The transcript about to be sent.
    pub transcript: &'a [Message],
    pub model: &'a str,
    /// The effort the request would go out with; `None` leaves it to
    /// the model.
    pub effort: Option<ReasoningEffort>,
    /// The turn the request is for.
    pub turn: u32,
}

/// Why the context is offered for a rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    /// A turn ended, and the run goes on.
    TurnEnd,
    /// The run starts on a transcript it inherited (a fork) or goes on
    /// with (a resumed run), before its first request: on another model,
    /// it may not fit.
    Start,
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
#[derive(Clone)]
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
    tools: Vec<Arc<dyn AgentTool>>,
    /// Per tool: the plugin that added it, and its schema once compiled.
    owners: Vec<PlanTool>,
    /// The plugin being started, which owns the tools it adds.
    starting: Option<usize>,
}

/// What the loop keeps of a [`RunPlan`] tool besides the tool.
#[derive(Clone)]
pub(crate) struct PlanTool {
    /// The index of the plugin that added it, among the agent's plugins.
    pub owner: Option<usize>,
    pub schema: Option<Arc<ArgumentSchema>>,
}

impl std::fmt::Debug for RunPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunPlan")
            .field("input", &self.input)
            .field("context", &self.context)
            .field("instructions", &self.instructions)
            .field("reasoning", &self.reasoning)
            .field("model", &self.model)
            .field("kind", &self.kind)
            .field("workflow", &self.workflow)
            .field(
                "tools",
                &self
                    .tools
                    .iter()
                    .map(|tool| tool.name())
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
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
            tools: Vec::new(),
            owners: Vec::new(),
            starting: None,
        }
    }

    /// Adds a tool for this run only, as tau-mcp adds the servers' direct
    /// tools. The tool counts as the plugin's being started: its calls
    /// get that plugin's context. A tool of the same name already in the
    /// plan is replaced, in its place. A `Nested` tool is callable from
    /// tools and not declared; any other is declared. The run's tools
    /// are fixed once its session opens.
    pub fn add_tool(&mut self, tool: Arc<dyn AgentTool>) {
        let owner = self.starting;
        self.push_tool(tool, owner, None);
    }

    /// The run's tools: the agent's, then the ones plugins added so far.
    pub fn tools(&self) -> &[Arc<dyn AgentTool>] {
        &self.tools
    }

    pub(crate) fn push_tool(
        &mut self,
        tool: Arc<dyn AgentTool>,
        owner: Option<usize>,
        schema: Option<Arc<ArgumentSchema>>,
    ) {
        let entry = PlanTool { owner, schema };
        match self.tools.iter().position(|t| t.name() == tool.name()) {
            Some(at) => {
                self.tools[at] = tool;
                self.owners[at] = entry;
            }
            None => {
                self.tools.push(tool);
                self.owners.push(entry);
            }
        }
    }

    /// The plan's tools with what the loop keeps of each.
    pub(crate) fn take_tools(&mut self) -> Vec<(Arc<dyn AgentTool>, PlanTool)> {
        std::mem::take(&mut self.tools)
            .into_iter()
            .zip(std::mem::take(&mut self.owners))
            .collect()
    }

    pub(crate) fn set_starting(&mut self, plugin: Option<usize>) {
        self.starting = plugin;
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
    /// [`Self::unsaved`] by plugin, in the order each first charged.
    pub unsaved_by: Vec<(Arc<str>, Usage)>,
    /// Charges the run has not emitted as [`RunEvent::PluginCharged`]
    /// yet.
    pub unreported: Vec<(Arc<str>, Usage)>,
}

impl Charged {
    fn add(&mut self, plugin: &Arc<str>, usage: &Usage) {
        self.total += usage;
        self.unsaved += usage;
        add_to(&mut self.unsaved_by, plugin, usage);
        self.unreported.push((plugin.clone(), usage.clone()));
    }

    /// What the store does not have yet, in all and by plugin, taken to
    /// be written.
    pub(crate) fn take_unsaved(&mut self) -> (Usage, Vec<(Arc<str>, Usage)>) {
        (
            std::mem::take(&mut self.unsaved),
            std::mem::take(&mut self.unsaved_by),
        )
    }

    /// Puts back what [`Self::take_unsaved`] took, when it could not be
    /// written, for the next write.
    pub(crate) fn untake(&mut self, all: &Usage, by: Vec<(Arc<str>, Usage)>) {
        self.unsaved += all;
        for (plugin, usage) in by {
            add_to(&mut self.unsaved_by, &plugin, &usage);
        }
    }
}

fn add_to(
    lines: &mut Vec<(Arc<str>, Usage)>,
    plugin: &Arc<str>,
    usage: &Usage,
) {
    match lines.iter_mut().find(|(name, _)| name == plugin) {
        Some((_, line)) => *line += usage,
        None => lines.push((plugin.clone(), usage.clone())),
    }
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
    reports: Reports,
}

/// Reports plugins made that the run has not emitted yet.
pub(crate) type Reports = Arc<Mutex<Vec<(Arc<str>, Value)>>>;

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
    pub reports: Reports,
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
            reports: self.reports.clone(),
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

    /// Asks the model once, outside the run's conversation: a session of
    /// its own (so its own lane, and the run's continuation untouched),
    /// with the run's retry policy, observing the run's cancellation.
    /// Every attempt's usage is charged to the run.
    ///
    /// Returns the final response, which can still be a failed one (a
    /// fatal error, or a retryable one past the policy): check its
    /// `stop_reason`. An error means no response at all: the session did
    /// not open, the run was cancelled, or the stream broke.
    pub async fn ask(
        &self,
        settings: Settings,
        input: &[Message],
    ) -> Result<AssistantMessage, AskError> {
        let mut session = self.llm.open(settings).await?;
        let mut attempts = 1;
        loop {
            let stream = session.respond(input, self.now());
            let (message, class) = match Reading::new(stream)
                .read_all(&self.cancel)
                .await
            {
                Streamed::Finished(message, class) => (message, class),
                Streamed::Cancelled(_) => return Err(AskError::Cancelled),
                Streamed::BrokeGrammar(_) => {
                    return Err(AskError::BrokeGrammar);
                }
                Streamed::NoTerminal(_) => return Err(AskError::NoTerminal),
            };
            self.charge(&message.usage);
            if class != Class::Retryable || !self.retry.allows(attempts) {
                return Ok(message);
            }
            let delay = self.retry.delay(attempts, jitter());
            attempts += 1;
            if !respond::wait(&self.cancel, delay).await {
                return Err(AskError::Cancelled);
            }
        }
    }

    /// Charges usage, cost included, to the run: it counts toward the
    /// run's limits and outcome, and is stored with the run's totals and
    /// as this plugin's cost. The run emits it as
    /// [`RunEvent::PluginCharged`] before its next event.
    pub fn charge(&self, usage: &Usage) {
        self.charged
            .lock()
            .expect("not poisoned")
            .add(&self.plugin, usage);
    }

    /// Reports what the plugin decided, for interfaces: the run emits it
    /// as [`RunEvent::PluginReport`] before its next event. Only
    /// subscribers see it; store what should outlast the run with
    /// [`Self::record`].
    pub fn report(&self, body: Value) {
        self.reports
            .lock()
            .expect("not poisoned")
            .push((self.plugin.clone(), body));
    }

    /// This plugin's records along the run's fork chain as stored now,
    /// oldest first: what [`RunPlan::records`] had at the start, plus
    /// what the run recorded since, and what others (an interface)
    /// stored for the plugin with the run meanwhile.
    pub async fn records(&self) -> Result<Vec<Value>, StoreError> {
        self.store
            .records(&self.run.0, &self.plugin)
            .await?
            .iter()
            .map(|body| serde_json::from_str(body).map_err(StoreError::Json))
            .collect()
    }

    /// Reports `record` and stores it with the run, in one call: what an
    /// interface shows of a plugin, live and from history alike (ADR
    /// 0017). The report goes out even when the record cannot be stored,
    /// and a record that is missing is said on stderr: an interface folds
    /// what it has. [`Self::try_publish`] says so to the caller instead.
    pub async fn publish(&self, record: &impl serde::Serialize) {
        if let Err(error) = self.try_publish(record).await {
            eprintln!("{}: a record could not be stored: {error}", self.plugin);
        }
    }

    /// [`Self::publish`], failing when the record is not stored.
    pub async fn try_publish(
        &self,
        record: &impl serde::Serialize,
    ) -> Result<(), StoreError> {
        let body = serde_json::to_value(record)?;
        self.report(body.clone());
        self.record(&body).await
    }

    /// Stores a record for this plugin with the run. The model never sees
    /// it. Forks of the run get it back in [`RunPlan::records`].
    pub async fn record(
        &self,
        record: &impl serde::Serialize,
    ) -> Result<(), StoreError> {
        let entry = Entry::Plugin {
            plugin: self.plugin.to_string(),
            body: serde_json::to_string(record)?,
        };
        let last = self
            .store
            .append_turn(&self.run.0, &[entry], TurnUsage::default())
            .await?;
        self.last_seq.fetch_max(last, Ordering::SeqCst);
        Ok(())
    }
}

/// Why [`PluginCtx::ask`] got no response at all.
#[derive(Debug, thiserror::Error)]
pub enum AskError {
    /// The session did not open.
    #[error(transparent)]
    Open(#[from] LlmError),
    #[error("the request was cancelled")]
    Cancelled,
    #[error("the response broke the event grammar")]
    BrokeGrammar,
    #[error("the response ended without a terminal event")]
    NoTerminal,
}
