//! `Agent` and `Run` (`docs/reference/api.md`).
//!
//! An [`Agent`] is a value: a model, instructions, tools, hooks and
//! limits, immutable once built and cheap to clone. [`Agent::start`]
//! starts a [`Run`], one execution with its own event stream, steering
//! and cancellation.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, atomic::AtomicI64},
};

use async_trait::async_trait;
use futures_util::{Stream, stream};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tau_ai::{
    llm::{Llm, LlmError},
    message::{
        AssistantBlock,
        AssistantMessage,
        InputBlock,
        Message,
        TextContent,
        ToolResultMessage,
        Usage,
        UserContent,
    },
    responses::request::{ReasoningEffort, Settings, ToolDefinition},
    retry::RetryPolicy,
};
use tau_store::{Entry, NewRun, RunKind, Status, Store, StoreError};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    event::{RunEvent, StopReason},
    hook::RunHook,
    limits::Limits,
    plugin::{Hooked, Plugin, RunPlan, RunShared},
    runner::{ActivePlugin, Clock, LoopTool, Runner, system_clock},
    schema::to_strict,
    tool::{AgentTool, RunId, ToolCtx, ToolError, ToolOutput},
    validation::ArgumentSchema,
};

/// How many events a run may get ahead of its subscriber before it
/// waits. A slow subscriber holds up its own run and no other.
const EVENT_BUFFER: usize = 64;

/// Why a run could not produce an outcome.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// The model provider could not open a session.
    #[error("model provider: {0}")]
    Llm(#[from] LlmError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// A tool's argument schema is not a valid JSON schema.
    #[error("tool {tool} has an invalid schema: {message}")]
    Schema { tool: String, message: String },
    /// The schema of a typed run's output type has no strict form.
    #[error("the output type has no strict schema: {0}")]
    OutputSchema(String),
    /// A typed run's final message is not a value of the output type.
    #[error(
        "the final message is not a valid output ({message}); the run stopped with {stop:?}",
        stop = outcome.stop
    )]
    Output {
        /// The run as it ended; its text is the message that failed.
        outcome: Box<Outcome>,
        message: String,
    },
    /// A plugin's `start` failed; the run did not start.
    #[error("plugin {plugin} could not start the run: {message}")]
    Plugin { plugin: String, message: String },
    /// The run's task panicked.
    #[error("the run's task panicked")]
    Panicked,
}

/// Why a sub-agent's call gave its caller no answer.
#[derive(Debug, thiserror::Error)]
pub enum SubAgentError {
    #[error("a sub-agent can only be called from a run")]
    NotInRun,
    #[error(transparent)]
    Agent(#[from] AgentError),
    /// The child stopped before finishing.
    #[error(
        "{name} ended with {stop:?} before finishing; its last message: {text}"
    )]
    Stopped {
        name: String,
        stop: StopReason,
        text: String,
    },
}

/// What a run starts from: the user's message, and optionally the
/// workflow the run belongs to. Runs started by a run (sub-agents)
/// inherit its workflow; so do forks, unless their input names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    pub workflow: Option<String>,
}

impl Input {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            workflow: None,
        }
    }

    /// Groups the run under `id`, for [`Store::workflow_cost`].
    pub fn workflow(mut self, id: impl Into<String>) -> Self {
        self.workflow = Some(id.into());
        self
    }
}

impl From<String> for Input {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for Input {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<&String> for Input {
    fn from(text: &String) -> Self {
        Self::new(text.clone())
    }
}

/// The result of a finished run.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub run: RunId,
    /// The text of the last assistant message.
    pub text: String,
    pub stop: StopReason,
    /// The model's usage over the run, cost included.
    pub usage: Usage,
    /// The `seq` of the run's last stored entry.
    last_seq: i64,
}

impl Outcome {
    /// The point the run ended at, to fork from.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            run: self.run.clone(),
            seq: self.last_seq,
        }
    }
}

/// A point in a run's transcript: its entries up to `seq`. Forks start
/// from one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Checkpoint {
    run: RunId,
    seq: i64,
}

impl Checkpoint {
    /// A checkpoint at `seq` in a stored run, such as one read back from
    /// the store: a fork from it inherits the run's entries up to `seq`.
    pub fn at(run: RunId, seq: i64) -> Self {
        Self { run, seq }
    }

    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// The `seq` of the last entry the checkpoint includes.
    pub fn seq(&self) -> i64 {
        self.seq
    }
}

/// The result of a typed run: the final message as a value, and the run.
#[derive(Debug, Clone, PartialEq)]
pub struct Typed<T> {
    pub value: T,
    pub outcome: Outcome,
}

impl<T: Serialize> Typed<T> {
    /// The value as JSON, for passing on to another agent.
    pub fn json(&self) -> String {
        serde_json::to_string(&self.value).expect("the value serializes")
    }
}

#[derive(Clone)]
struct AgentInner {
    llm: Arc<dyn Llm>,
    name: Arc<str>,
    model: String,
    instructions: Option<String>,
    reasoning: Option<ReasoningEffort>,
    tools: Vec<Arc<dyn AgentTool>>,
    /// Plugins and hooks, in registration order.
    plugins: Vec<Arc<dyn Plugin>>,
    limits: Limits,
    retry: RetryPolicy,
    warmup: bool,
    clock: Clock,
}

/// An agent: what to run, not a running thing.
#[derive(Clone)]
pub struct Agent(Arc<AgentInner>);

impl fmt::Debug for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agent")
            .field("name", &self.0.name)
            .field("model", &self.0.model)
            .field(
                "tools",
                &self.0.tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Agent {
    pub fn new(llm: impl Llm) -> Self {
        Self(Arc::new(AgentInner {
            llm: Arc::new(llm),
            name: "agent".into(),
            model: "gpt-5.5".into(),
            instructions: None,
            reasoning: None,
            tools: Vec::new(),
            plugins: Vec::new(),
            limits: Limits::default(),
            retry: RetryPolicy::default(),
            warmup: false,
            clock: system_clock(),
        }))
    }

    fn with(self, change: impl FnOnce(&mut AgentInner)) -> Self {
        let mut inner = Arc::unwrap_or_clone(self.0);
        change(&mut inner);
        Self(Arc::new(inner))
    }

    pub fn name(self, name: &str) -> Self {
        self.with(|a| a.name = name.into())
    }

    pub fn model(self, id: &str) -> Self {
        self.with(|a| a.model = id.into())
    }

    pub fn instructions(self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.with(|a| a.instructions = Some(text))
    }

    pub fn reasoning(self, effort: ReasoningEffort) -> Self {
        self.with(|a| a.reasoning = Some(effort))
    }

    pub fn tool(self, tool: impl AgentTool) -> Self {
        self.with(|a| a.tools.push(Arc::new(tool)))
    }

    /// Adds several tools at once, such as a toolkit's.
    pub fn tools(
        self,
        tools: impl IntoIterator<Item = Arc<dyn AgentTool>>,
    ) -> Self {
        self.with(|a| a.tools.extend(tools))
    }

    /// Adds a hook. It runs in registration order among the plugins,
    /// and every run shares it.
    pub fn hook(self, hook: impl RunHook) -> Self {
        self.with(|a| a.plugins.push(Arc::new(Hooked(Arc::new(hook)))))
    }

    /// Adds a plugin (`docs/reference/plugins.md`): its tools join the
    /// agent's, and each run starts it after the plugins added before it.
    pub fn plugin(self, plugin: impl Plugin) -> Self {
        self.with(|a| {
            a.tools.extend(plugin.tools());
            a.plugins.push(Arc::new(plugin));
        })
    }

    pub fn limits(self, limits: Limits) -> Self {
        self.with(|a| a.limits = limits)
    }

    /// How failed responses are retried (`docs/reference/agent-loop.md`,
    /// "Retries"). Defaults to 3 attempts with a 2 s base.
    pub fn retry(self, policy: RetryPolicy) -> Self {
        self.with(|a| a.retry = policy)
    }

    /// Warms each run's session up before its first turn
    /// (`docs/reference/openai-websocket.md`, "Warm-up"): OpenAI prepares
    /// the instructions and tools, and the first turn continues from
    /// that. Off by default.
    pub fn warmup(self, on: bool) -> Self {
        self.with(|a| a.warmup = on)
    }

    /// Replaces the clock that stamps messages, for deterministic tests.
    pub fn clock(self, clock: Clock) -> Self {
        self.with(|a| a.clock = clock)
    }

    /// The settings a run of this agent sends, as its plan left them.
    /// Tool schemas go in strict form when they convert; otherwise as
    /// they are, with `strict: false`.
    fn settings(&self, plan: &RunPlan, text_format: Option<Value>) -> Settings {
        let tools = self
            .0
            .tools
            .iter()
            .map(|tool| {
                let (parameters, strict) = match to_strict(tool.parameters()) {
                    Ok(strict) => (strict, true),
                    Err(_) => (tool.parameters().clone(), false),
                };
                ToolDefinition {
                    name: tool.name().to_owned(),
                    description: tool.description().to_owned(),
                    parameters,
                    strict,
                }
            })
            .collect();
        Settings {
            model: self.0.model.clone(),
            instructions: plan.instructions.clone(),
            tools,
            reasoning: plan.reasoning,
            text_format,
            ..Settings::default()
        }
    }

    /// Starts a run on `input`. Must be called inside a tokio runtime.
    pub fn start(&self, input: impl Into<Input>, store: &Store) -> Run {
        let input = input.into();
        self.launch(Launch::root(input.workflow.as_deref()), input.text, store)
    }

    /// Runs `input` to the end and returns the outcome.
    pub async fn run(
        &self,
        input: impl Into<Input>,
        store: &Store,
    ) -> Result<Outcome, AgentError> {
        self.start(input, store).outcome().await
    }

    /// Runs `input` to the end with the final message constrained to the
    /// JSON schema of `T`, and parses it. Tools stay available; only the
    /// final message must be a `T`.
    pub async fn run_typed<T>(
        &self,
        input: impl Into<Input>,
        store: &Store,
    ) -> Result<Typed<T>, AgentError>
    where
        T: DeserializeOwned + JsonSchema,
    {
        let input = input.into();
        let mut launch = Launch::root(input.workflow.as_deref());
        launch.text_format = Some(text_format::<T>()?);
        let outcome = self.launch(launch, input.text, store).outcome().await?;
        parse_output(outcome)
    }

    /// Continues from `checkpoint` in a new run of this agent: a fork.
    /// The fork sees the checkpoint's transcript, by reference, and its
    /// input follows it; the forked-from run is left as it was.
    pub fn fork(&self, checkpoint: &Checkpoint) -> Forked {
        Forked {
            agent: self.clone(),
            from: checkpoint.clone(),
            after_turn: 0,
        }
    }

    /// Goes on with `run`, a finished run of this agent, as a chat goes
    /// on: its input follows the run's transcript, in the same run, whose
    /// turns keep counting and whose usage keeps adding up. Limits apply
    /// to the turns of this start alone. Fails with
    /// [`StoreError::StillRunning`] if the run has not finished.
    pub fn resume(&self, run: &RunId) -> Resumed {
        Resumed {
            agent: self.clone(),
            run: run.clone(),
        }
    }

    /// This agent as a tool: each call starts a sub-agent run on the
    /// call's `input`, and its final text is the tool's result.
    ///
    /// The sub-agent run records the calling run as its parent and joins
    /// its workflow. Cancelling the calling run cancels it; its usage
    /// counts toward the calling run's limits and outcome; its events
    /// go to the calling run's subscriber, carrying `parent`. A run that
    /// ends other than by stopping (a limit, a cancel, an error) makes
    /// the call fail.
    pub fn as_tool(&self, name: &str, description: &str) -> SubAgent {
        SubAgent {
            agent: self.clone(),
            fork: false,
            name: name.to_owned(),
            description: description.to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "The task, with everything needed to do it.",
                    },
                },
                "required": ["input"],
                "additionalProperties": false,
            }),
        }
    }

    /// Starts a run as `launch` describes it.
    fn launch(&self, launch: Launch, input: String, store: &Store) -> Run {
        let id = launch
            .resume
            .clone()
            .unwrap_or_else(|| RunId(uuid::Uuid::now_v7().to_string().into()));
        let (own_tx, own_rx) = mpsc::channel(EVENT_BUFFER);
        let (events_tx, events) = match launch.events.clone() {
            Events::Own => (Some(own_tx), Some(own_rx)),
            Events::Forward(sender) => (sender, None),
        };
        let (steer_tx, steering) = mpsc::unbounded_channel();
        let cancel = launch.cancel.clone();
        let task = tokio::spawn(run_task(
            self.clone(),
            id.clone(),
            input,
            store.clone(),
            launch,
            events_tx,
            steering,
        ));
        Run {
            id,
            events,
            steer: steer_tx,
            cancel,
            task,
        }
    }
}

/// An agent about to continue from a checkpoint. See [`Agent::fork`].
#[derive(Debug, Clone)]
pub struct Forked {
    agent: Agent,
    from: Checkpoint,
    after_turn: u32,
}

impl Forked {
    /// The checkpoint ends the parent's `turn`th turn: the fork's turns
    /// count on from it, as the conversation does. Without it they count
    /// from 1.
    pub fn after_turn(mut self, turn: u32) -> Self {
        self.after_turn = turn;
        self
    }

    /// Starts the fork on `input`. Unless the input names a workflow,
    /// the fork joins the workflow of the run it forks from.
    pub fn start(&self, input: impl Into<Input>, store: &Store) -> Run {
        let input = input.into();
        self.agent.launch(self.launch(&input), input.text, store)
    }

    /// Runs the fork to the end. Takes the fork by value, so
    /// `agent.fork(&cp).run(..)` can be collected and awaited later.
    pub async fn run(
        self,
        input: impl Into<Input>,
        store: &Store,
    ) -> Result<Outcome, AgentError> {
        self.start(input, store).outcome().await
    }

    pub async fn run_typed<T>(
        self,
        input: impl Into<Input>,
        store: &Store,
    ) -> Result<Typed<T>, AgentError>
    where
        T: DeserializeOwned + JsonSchema,
    {
        let input = input.into();
        let mut launch = self.launch(&input);
        launch.text_format = Some(text_format::<T>()?);
        let outcome = self
            .agent
            .launch(launch, input.text, store)
            .outcome()
            .await?;
        parse_output(outcome)
    }

    fn launch(&self, input: &Input) -> Launch {
        let mut launch = Launch::root(input.workflow.as_deref());
        launch.kind = RunKind::Fork {
            parent: self.from.run.0.to_string(),
            fork_seq: self.from.seq,
        };
        launch.inherit_workflow = input.workflow.is_none();
        launch.turns_before = self.after_turn;
        launch
    }
}

/// An agent about to go on with a finished run. See [`Agent::resume`].
#[derive(Debug, Clone)]
pub struct Resumed {
    agent: Agent,
    run: RunId,
}

impl Resumed {
    /// Starts the run again on `input`.
    pub fn start(&self, input: impl Into<Input>, store: &Store) -> Run {
        let input = input.into();
        let mut launch = Launch::root(input.workflow.as_deref());
        launch.resume = Some(self.run.clone());
        self.agent.launch(launch, input.text, store)
    }

    /// Runs it to the end.
    pub async fn run(
        self,
        input: impl Into<Input>,
        store: &Store,
    ) -> Result<Outcome, AgentError> {
        self.start(input, store).outcome().await
    }
}

/// An agent as a tool. See [`Agent::as_tool`].
pub struct SubAgent {
    agent: Agent,
    /// Whether each call forks the calling run.
    fork: bool,
    name: String,
    description: String,
    parameters: Value,
}

#[async_trait]
impl AgentTool for SubAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        Ok(self.ask(args, ctx).await?)
    }
}

impl SubAgent {
    /// Each call forks the calling run instead of starting blank. The
    /// sub-agent inherits the caller's stored transcript, then the turn
    /// that made the call with an output for each of its calls (see
    /// `fork_prelude`), then its input.
    pub fn forking(mut self) -> Self {
        self.fork = true;
        self
    }

    /// Runs the child to its end, and answers with its last text.
    async fn ask(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, SubAgentError> {
        let Some(scope) = ctx.scope else {
            return Err(SubAgentError::NotInRun);
        };
        let input = args["input"].as_str().unwrap_or_default().to_owned();
        let (fork_seq, prelude) = if self.fork {
            let prelude = fork_prelude(&scope.turn, &scope.call, &self.name);
            (Some(scope.stored), prelude)
        } else {
            (None, Vec::new())
        };
        let launch = Launch {
            kind: RunKind::Subagent {
                parent: ctx.run.0.to_string(),
                fork_seq,
            },
            parent: Some(ctx.run.clone()),
            workflow: scope.workflow.clone(),
            cancel: ctx.cancel.child_token(),
            text_format: None,
            inherit_workflow: false,
            events: Events::Forward(scope.events.clone()),
            resume: None,
            turns_before: 0,
            prelude,
        };
        let outcome = self
            .agent
            .launch(launch, input, &scope.store)
            .outcome()
            .await?;
        *scope.children.lock().expect("not poisoned") += &outcome.usage;
        match &outcome.stop {
            StopReason::Stop => Ok(ToolOutput {
                details: Some(json!({ "run": outcome.run.0.as_ref() })),
                ..ToolOutput::text(outcome.text)
            }),
            stop => Err(SubAgentError::Stopped {
                name: self.name.clone(),
                stop: stop.clone(),
                text: outcome.text,
            }),
        }
    }
}

impl fmt::Debug for SubAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubAgent")
            .field("name", &self.name)
            .field("agent", &self.agent)
            .finish()
    }
}

/// How a run relates to the rest of its workflow.
struct Launch {
    kind: RunKind,
    parent: Option<RunId>,
    workflow: Option<Arc<str>>,
    cancel: CancellationToken,
    /// The `text.format` of a typed run.
    text_format: Option<Value>,
    /// Whether the run joins its parent run's workflow.
    inherit_workflow: bool,
    events: Events,
    /// Go on with this stored run instead of starting a new one.
    resume: Option<RunId>,
    /// Turns a new run starts after: a fork's, from its parent.
    turns_before: u32,
    /// Messages a new run stores as its own before its input.
    prelude: Vec<Message>,
}

/// Where a run's events go.
#[derive(Clone)]
enum Events {
    /// To its own subscriber, through [`Run::events`].
    Own,
    /// To its parent's subscriber, if the parent still has one.
    Forward(Option<mpsc::Sender<RunEvent>>),
}

impl Launch {
    fn root(workflow: Option<&str>) -> Self {
        Self {
            kind: RunKind::Root,
            parent: None,
            workflow: workflow.map(Into::into),
            cancel: CancellationToken::new(),
            text_format: None,
            inherit_workflow: false,
            events: Events::Own,
            resume: None,
            turns_before: 0,
            prelude: Vec::new(),
        }
    }
}

/// The Responses `text.format` that constrains the final message to
/// `T`: its schema in strict form, named after the type.
fn text_format<T: JsonSchema>() -> Result<Value, AgentError> {
    let schema = serde_json::to_value(schemars::schema_for!(T))
        .expect("a generated schema is valid JSON");
    let strict = to_strict(&schema)
        .map_err(|error| AgentError::OutputSchema(error.to_string()))?;
    Ok(json!({
        "type": "json_schema",
        "name": format_name(&T::schema_name()),
        "schema": strict,
        "strict": true,
    }))
}

/// A `text.format` name: 1–64 characters from `[A-Za-z0-9_-]`.
fn format_name(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if name.is_empty() {
        "output".to_owned()
    } else {
        name
    }
}

/// Parses a typed run's final message.
fn parse_output<T: DeserializeOwned>(
    outcome: Outcome,
) -> Result<Typed<T>, AgentError> {
    match serde_json::from_str(&outcome.text) {
        Ok(value) => Ok(Typed { value, outcome }),
        Err(error) => Err(AgentError::Output {
            outcome: Box::new(outcome),
            message: error.to_string(),
        }),
    }
}

async fn run_task(
    agent: Agent,
    id: RunId,
    input: String,
    store: Store,
    launch: Launch,
    events: Option<mpsc::Sender<RunEvent>>,
    steering: mpsc::UnboundedReceiver<String>,
) -> Result<Outcome, AgentError> {
    let mut tools = HashMap::new();
    for tool in &agent.0.tools {
        let schema =
            ArgumentSchema::new(tool.parameters()).map_err(|error| {
                AgentError::Schema {
                    tool: tool.name().to_owned(),
                    message: error.to_string(),
                }
            })?;
        tools.insert(
            tool.name().to_owned(),
            LoopTool {
                tool: tool.clone(),
                schema: Arc::new(schema),
            },
        );
    }
    // A resumed run is reopened as it was stored: its kind, workflow and
    // turns. It goes on on this agent's model, which may be another.
    let resumed = match &launch.resume {
        Some(run) => Some(store.reopen_run(&run.0, &agent.0.model).await?),
        None => None,
    };
    let kind = resumed
        .as_ref()
        .map_or_else(|| launch.kind.clone(), |record| record.kind.clone());
    let mut workflow = launch.workflow.as_deref().map(str::to_owned);
    let parent_run = match &launch.kind {
        RunKind::Root => None,
        RunKind::Fork { parent, .. } | RunKind::Subagent { parent, .. } => {
            Some(parent.clone())
        }
    };
    if launch.inherit_workflow
        && let Some(parent) = &parent_run
    {
        let record = store.run(parent).await?;
        let record = record.ok_or_else(|| {
            AgentError::Store(StoreError::UnknownRun(parent.clone()))
        })?;
        workflow = record.workflow_id;
    }
    let fork = matches!(
        launch.kind,
        RunKind::Fork { .. }
            | RunKind::Subagent {
                fork_seq: Some(_),
                ..
            }
    );
    match &resumed {
        Some(record) => workflow = record.workflow_id.clone(),
        None => {
            store
                .create_run(&NewRun {
                    id: &id.0,
                    workflow_id: workflow.as_deref(),
                    agent: &agent.0.name,
                    kind: launch.kind.clone(),
                    model: &agent.0.model,
                    turns: i64::from(launch.turns_before),
                })
                .await?
        }
    }
    let turns_before = resumed.as_ref().map_or(launch.turns_before, |record| {
        u32::try_from(record.turns).unwrap_or(u32::MAX)
    });
    let workflow: Option<Arc<str>> = workflow.map(Into::into);

    let shared = RunShared {
        run: id.clone(),
        parent: launch.parent.clone(),
        agent: agent.0.name.clone(),
        cancel: launch.cancel.clone(),
        llm: agent.0.llm.clone(),
        store: store.clone(),
        charged: Arc::default(),
        last_seq: Arc::new(AtomicI64::new(-1)),
        clock: agent.0.clock.clone(),
        retry: agent.0.retry,
        reports: Arc::default(),
    };
    // A fork starts from its inherited transcript, a resumed run from
    // its own, and from the latest context rewrite in it, if any.
    let (mut history, last_rewrite) = if fork || resumed.is_some() {
        let entries = match store.transcript(&id.0).await {
            Ok(entries) => entries,
            Err(error) => {
                return Err(fail(&store, &id, AgentError::Store(error)).await);
            }
        };
        match messages(entries) {
            Ok(loaded) => loaded,
            Err(error) => {
                return Err(fail(&store, &id, AgentError::Store(error)).await);
            }
        }
    } else {
        (Vec::new(), None)
    };
    let mut prelude = launch.prelude;
    keep_own_reasoning(&mut history, &agent.0.model);
    keep_own_reasoning(&mut prelude, &agent.0.model);
    let mut plan = RunPlan::new(
        input,
        agent.0.instructions.clone(),
        agent.0.reasoning,
        agent.0.model.clone(),
        kind,
        workflow.clone(),
    );
    let mut plugins = Vec::with_capacity(agent.0.plugins.len());
    for plugin in &agent.0.plugins {
        let ctx = shared.ctx(plugin.name());
        let records = match plugin_records(&store, &id, plugin.name()).await {
            Ok(records) => records,
            Err(error) => return Err(fail(&store, &id, error).await),
        };
        plan.set_records(records);
        plan.set_last_rewrite(
            last_rewrite
                .as_ref()
                .filter(|(by, _)| by == plugin.name())
                .map(|(_, details)| details.clone()),
        );
        match plugin.start(&mut plan, &ctx).await {
            Ok(run) => plugins.push(ActivePlugin { run, ctx }),
            Err(error) => {
                let error = AgentError::Plugin {
                    plugin: plugin.name().to_owned(),
                    message: error.to_string(),
                };
                return Err(fail(&store, &id, error).await);
            }
        }
    }

    let settings = agent.settings(&plan, launch.text_format);
    let session = match agent.0.llm.open(settings).await {
        Ok(session) => session,
        Err(error) => {
            return Err(fail(&store, &id, AgentError::Llm(error)).await);
        }
    };
    let first = first_message(plan.context, plan.input);
    let result = Runner {
        run: id.clone(),
        parent: launch.parent,
        agent: agent.0.name.clone(),
        tools,
        plugins,
        limits: agent.0.limits,
        session,
        store,
        events,
        steering,
        cancel: launch.cancel,
        clock: agent.0.clock.clone(),
        history,
        prelude,
        pending_turn: None,
        reports: shared.reports,
        last_seq: shared.last_seq,
        charged: shared.charged,
        workflow,
        children: Arc::default(),
        retry: agent.0.retry,
        warmup: agent.0.warmup,
        turns_before,
    }
    .run(first)
    .await?;
    Ok(Outcome {
        run: id,
        text: result.text,
        stop: result.stop,
        usage: result.usage,
        last_seq: result.last_seq,
    })
}

/// A plugin's records along the run's fork chain, parsed.
async fn plugin_records(
    store: &Store,
    run: &RunId,
    plugin: &str,
) -> Result<Vec<Value>, AgentError> {
    let bodies = store.records(&run.0, plugin).await?;
    bodies
        .iter()
        .map(|body| serde_json::from_str(body))
        .collect::<Result<_, _>>()
        .map_err(|error| AgentError::Store(StoreError::Json(error)))
}

/// Marks a run that could not start as failed, and returns why. If even
/// that write fails, the store's error wins: the run is left `running`.
async fn fail(store: &Store, run: &RunId, error: AgentError) -> AgentError {
    let message = error.to_string();
    match store
        .finish_run(&run.0, Status::Failed, None, Some(&message))
        .await
    {
        Ok(()) => error,
        Err(store_error) => AgentError::Store(store_error),
    }
}

/// The run's first user message: the plan's context, then its input. With
/// no context, it is the input alone, as plain text.
fn first_message(context: Vec<String>, input: String) -> UserContent {
    if context.is_empty() {
        return UserContent::Text(input);
    }
    UserContent::Blocks(
        context
            .into_iter()
            .chain(std::iter::once(input))
            .map(|text| {
                InputBlock::Text(TextContent {
                    text,
                    text_signature: None,
                })
            })
            .collect(),
    )
}

/// What a forking sub-agent stores before its input: its caller's turn,
/// which the caller stores only once its tools are done, and an output
/// for each of that turn's calls, so the sub-agent sees the whole batch
/// and knows its own part in it. `tool` is the sub-agent's tool name:
/// calls to it are its siblings.
fn fork_prelude(
    turn: &AssistantMessage,
    call: &str,
    tool: &str,
) -> Vec<Message> {
    let outputs = turn.content.iter().filter_map(|block| match block {
        AssistantBlock::ToolCall(each) => {
            let text = if each.id == call {
                "You are the sub-agent running this call. Your task \
                 follows: do that task alone, then answer with what you \
                 did."
            } else if each.name == tool {
                "Another sub-agent runs this call."
            } else {
                "This call's result is not available to you."
            };
            Some(Message::ToolResult(ToolResultMessage {
                tool_call_id: each.id.clone(),
                tool_name: each.name.clone(),
                content: vec![InputBlock::Text(TextContent {
                    text: text.to_owned(),
                    text_signature: None,
                })],
                details: None,
                is_error: false,
                timestamp: turn.timestamp,
            }))
        }
        _ => None,
    });
    std::iter::once(Message::Assistant(turn.clone()))
        .chain(outputs)
        .collect()
}

/// Drops the reasoning of the assistant messages another model wrote,
/// and keeps the rest of them: reasoning goes back only to the model
/// that wrote it, as a fork or a resumed run on another model would
/// otherwise send it.
fn keep_own_reasoning(messages: &mut [Message], model: &str) {
    for message in messages {
        if let Message::Assistant(assistant) = message
            && assistant.model != model
        {
            assistant
                .content
                .retain(|block| !matches!(block, AssistantBlock::Thinking(_)));
        }
    }
}

/// The latest context rewrite in a transcript: the plugin that made it,
/// and its details.
type LatestRewrite = (String, Value);

/// The messages of a stored transcript, and the latest context rewrite
/// in it, if any: the plugin that made it, and its details.
///
/// A context entry is followed by the messages it rewrote the transcript
/// to.
fn messages(
    entries: Vec<Entry>,
) -> Result<(Vec<Message>, Option<LatestRewrite>), StoreError> {
    let mut rewrite = None;
    let mut messages = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            Entry::Message { body, .. } => {
                messages.push(serde_json::from_str(&body)?);
            }
            Entry::Context { plugin, body } => {
                rewrite = Some((plugin, serde_json::from_str(&body)?));
            }
            // `Store::transcript` leaves plugin records out.
            Entry::Plugin { .. } => {}
        }
    }
    Ok((messages, rewrite))
}

/// One execution of an agent.
pub struct Run {
    id: RunId,
    events: Option<mpsc::Receiver<RunEvent>>,
    steer: mpsc::UnboundedSender<String>,
    cancel: CancellationToken,
    task: JoinHandle<Result<Outcome, AgentError>>,
}

impl Run {
    pub fn id(&self) -> RunId {
        self.id.clone()
    }

    /// The run's events, in order, until `RunEnd`.
    pub fn events(&mut self) -> impl Stream<Item = RunEvent> + '_ {
        stream::poll_fn(move |cx| match &mut self.events {
            Some(events) => events.poll_recv(cx),
            None => std::task::Poll::Ready(None),
        })
    }

    /// Queues a user message for the run. It is added after the current
    /// tool batch.
    pub fn steer(&self, message: impl Into<String>) {
        let _ = self.steer.send(message.into());
    }

    /// Cancels the run. Running tools see the cancellation token.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// A handle that steers and cancels the run from elsewhere, such as a
    /// UI, while a task reads its events.
    pub fn control(&self) -> RunControl {
        RunControl {
            id: self.id.clone(),
            steer: self.steer.clone(),
            cancel: self.cancel.clone(),
        }
    }

    /// Waits for the run to end. Events not yet read are dropped, so a
    /// run nobody listens to never waits for a subscriber.
    pub async fn outcome(mut self) -> Result<Outcome, AgentError> {
        self.events = None;
        match (&mut self.task).await {
            Ok(result) => result,
            Err(_) => Err(AgentError::Panicked),
        }
    }
}

/// Steers and cancels a run without owning it; see [`Run::control`].
/// Cheap to clone. Does nothing once the run has ended.
#[derive(Clone)]
pub struct RunControl {
    id: RunId,
    steer: mpsc::UnboundedSender<String>,
    cancel: CancellationToken,
}

impl RunControl {
    pub fn id(&self) -> RunId {
        self.id.clone()
    }

    /// Like [`Run::steer`].
    pub fn steer(&self, message: impl Into<String>) {
        let _ = self.steer.send(message.into());
    }

    /// Like [`Run::cancel`].
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl fmt::Debug for RunControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunControl")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Run {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Run")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Characters OpenAI does not allow in a format name become `_`,
    /// `-` and `_` stay, the name is cut to 64 characters, and an empty
    /// name falls back to `output`.
    #[test]
    fn format_names_are_valid() {
        assert_eq!(format_name("Page_for_bool"), "Page_for_bool");
        assert_eq!(format_name("a-b<c>.d"), "a-b_c__d");
        assert_eq!(format_name(&"x".repeat(70)), "x".repeat(64));
        assert_eq!(format_name(""), "output");
    }

    /// Any name becomes a valid format name (1–64 characters from
    /// `[A-Za-z0-9_-]`), a second pass changes nothing, and a name that is
    /// already valid is kept as it is.
    #[hegel::test(test_cases = 300)]
    fn format_names_are_valid_for_any_name(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let valid = |name: &str| {
            (1..=64).contains(&name.len())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        let name: String = tc.draw(gs::text().max_size(80));
        let formatted = format_name(&name);
        assert!(valid(&formatted), "{name:?} became {formatted:?}");
        assert_eq!(format_name(&formatted), formatted);
        let already: String =
            tc.draw(gs::from_regex("[A-Za-z0-9_-]{1,64}").fullmatch(true));
        assert_eq!(format_name(&already), already);
    }

    /// Only another model's reasoning goes: this model's messages, and
    /// every other message, stay as they were, and the other blocks of a
    /// foreign message keep their order.
    #[hegel::test(test_cases = 300)]
    fn only_another_models_reasoning_is_dropped(tc: hegel::TestCase) {
        use hegel::generators as gs;
        use tau_testing::generators::message;
        let before: Vec<Message> = tc.draw(gs::vecs(message()).max_size(8));
        let model: String = tc.draw(gs::sampled_from(vec![
            "gpt-5.5".to_owned(),
            "gpt-5.5-mini".to_owned(),
        ]));
        let mut after = before.clone();
        keep_own_reasoning(&mut after, &model);
        assert_eq!(after.len(), before.len());
        for (was, now) in before.iter().zip(&after) {
            match (was, now) {
                (Message::Assistant(was), Message::Assistant(now))
                    if was.model != model =>
                {
                    let kept: Vec<&AssistantBlock> = was
                        .content
                        .iter()
                        .filter(|b| !matches!(b, AssistantBlock::Thinking(_)))
                        .collect();
                    assert_eq!(now.content.iter().collect::<Vec<_>>(), kept);
                    assert_eq!(
                        AssistantMessage {
                            content: Vec::new(),
                            ..now.clone()
                        },
                        AssistantMessage {
                            content: Vec::new(),
                            ..was.clone()
                        },
                    );
                }
                _ => assert_eq!(now, was),
            }
        }
    }
}
