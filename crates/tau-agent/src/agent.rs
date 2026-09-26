//! `Agent` and `Run` (`docs/reference/api.md`).
//!
//! An [`Agent`] is a value: a model, instructions, tools, hooks and
//! limits, immutable once built and cheap to clone. [`Agent::start`]
//! starts a [`Run`], one execution with its own event stream, steering
//! and cancellation.

use std::{collections::HashMap, fmt, sync::Arc};

use futures_util::{Stream, stream};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tau_ai::{
    llm::{Llm, LlmError},
    message::Usage,
    responses::request::{ReasoningEffort, Settings, ToolDefinition},
};
use tau_store::{NewRun, RunKind, Store, StoreError};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    event::{RunEvent, StopReason},
    hook::RunHook,
    limits::Limits,
    runner::{Clock, LoopTool, Runner, system_clock},
    schema::to_strict,
    tool::{AgentTool, RunId},
    validation::ArgumentSchema,
};

/// How many events a run may get ahead of its subscriber before it
/// waits. A slow subscriber holds up its own run and no other.
const EVENT_BUFFER: usize = 64;

/// Why a run could not produce an outcome.
#[derive(Debug)]
pub enum AgentError {
    /// The model provider could not open a session.
    Llm(LlmError),
    Store(StoreError),
    /// A tool's argument schema is not a valid JSON schema.
    Schema {
        tool: String,
        message: String,
    },
    /// The schema of a typed run's output type has no strict form.
    OutputSchema(String),
    /// A typed run's final message is not a value of the output type.
    Output {
        /// The run as it ended; its text is the message that failed.
        outcome: Box<Outcome>,
        message: String,
    },
    /// The run's task panicked.
    Panicked,
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Llm(error) => write!(f, "model provider: {error}"),
            Self::Store(error) => write!(f, "store: {error}"),
            Self::Schema { tool, message } => {
                write!(f, "tool {tool} has an invalid schema: {message}")
            }
            Self::OutputSchema(message) => {
                write!(f, "the output type has no strict schema: {message}")
            }
            Self::Output { outcome, message } => write!(
                f,
                "the final message is not a valid output ({message}); the run stopped with {:?}",
                outcome.stop
            ),
            Self::Panicked => f.write_str("the run's task panicked"),
        }
    }
}

impl std::error::Error for AgentError {}

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
    hooks: Vec<Arc<dyn RunHook>>,
    limits: Limits,
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
            hooks: Vec::new(),
            limits: Limits::default(),
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

    pub fn hook(self, hook: impl RunHook) -> Self {
        self.with(|a| a.hooks.push(Arc::new(hook)))
    }

    pub fn limits(self, limits: Limits) -> Self {
        self.with(|a| a.limits = limits)
    }

    /// Replaces the clock that stamps messages, for deterministic tests.
    pub fn clock(self, clock: Clock) -> Self {
        self.with(|a| a.clock = clock)
    }

    /// The settings a run of this agent sends. Tool schemas go in strict
    /// form when they convert; otherwise as they are, with `strict: false`.
    fn settings(&self, text_format: Option<Value>) -> Settings {
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
            instructions: self.0.instructions.clone(),
            tools,
            reasoning: self.0.reasoning,
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

    /// Starts a run as `launch` describes it.
    fn launch(&self, launch: Launch, input: String, store: &Store) -> Run {
        let id = RunId(uuid::Uuid::now_v7().to_string().into());
        let (events_tx, events) = mpsc::channel(EVENT_BUFFER);
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
            events: Some(events),
            steer: steer_tx,
            cancel,
            task,
        }
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
}

impl Launch {
    fn root(workflow: Option<&str>) -> Self {
        Self {
            kind: RunKind::Root,
            parent: None,
            workflow: workflow.map(Into::into),
            cancel: CancellationToken::new(),
            text_format: None,
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
    events: mpsc::Sender<RunEvent>,
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
    let settings = agent.settings(launch.text_format);
    let session = agent.0.llm.open(settings).await.map_err(AgentError::Llm)?;
    store
        .create_run(&NewRun {
            id: &id.0,
            workflow_id: launch.workflow.as_deref(),
            agent: &agent.0.name,
            kind: launch.kind,
            model: &agent.0.model,
        })
        .await
        .map_err(AgentError::Store)?;
    let result = Runner {
        run: id.clone(),
        parent: launch.parent,
        agent: agent.0.name.clone(),
        tools,
        hooks: agent.0.hooks.clone(),
        limits: agent.0.limits,
        session,
        store,
        events: Some(events),
        steering,
        cancel: launch.cancel,
        clock: agent.0.clock.clone(),
    }
    .run(input)
    .await
    .map_err(AgentError::Store)?;
    Ok(Outcome {
        run: id,
        text: result.text,
        stop: result.stop,
        usage: result.usage,
    })
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
}
