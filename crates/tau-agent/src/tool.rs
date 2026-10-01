//! Tools an agent can call.
//!
//! [`AgentTool`] is the object-safe interface the loop runs.
//! [`TypedTool`] is the convenient one: its arguments are a Rust type
//! whose JSON schema comes from `schemars`; wrap it with [`typed`] to get
//! an `AgentTool`.
//!
//! A tool receives a [`ToolCtx`] with the run's cancellation token, which
//! it must observe itself: the loop never drops a running tool's future
//! (`docs/reference/agent-loop.md`, "Cancellation").
//!
//! A tool can call other tools through the loop with [`ToolCtx::call`]
//! (`docs/reference/plugins.md`, "Nested calls"). Which tools the model
//! sees and which tools can call them is the tool's [`Exposure`].

use std::{
    fmt,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tau_ai::message::{AssistantMessage, InputBlock, TextContent, Usage};
use tau_store::Store;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub use crate::error::ToolError;
use crate::{event::RunEvent, plugin::PluginCtx, runner::Toolbox};

/// What a tool returns.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ToolOutput {
    /// What the model sees.
    pub content: Vec<InputBlock>,
    /// Tool-specific details for callers and hooks; the model never sees
    /// them.
    pub details: Option<Value>,
    /// What a calling tool gets instead of the text, shaped by
    /// [`AgentTool::output_schema`]; the model never sees it, and the
    /// transcript does not keep it.
    pub structured: Option<Value>,
}

impl ToolOutput {
    /// A text-only output.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![InputBlock::Text(TextContent {
                text: text.into(),
                text_signature: None,
            })],
            details: None,
            structured: None,
        }
    }
}

/// Who sees a tool: the model, other tools, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Exposure {
    /// Declared to the model, and callable from tools.
    #[default]
    Direct,
    /// Callable from tools only: never declared, so it costs nothing
    /// against the delta rule.
    Nested,
    /// Declared to the model, never callable from tools, as `codemode`
    /// is, so a script cannot start a script.
    ModelOnly,
}

impl Exposure {
    /// Whether the model is told about the tool.
    pub fn declared(self) -> bool {
        matches!(self, Self::Direct | Self::ModelOnly)
    }

    /// Whether another tool can call it with [`ToolCtx::call`].
    pub fn callable(self) -> bool {
        matches!(self, Self::Direct | Self::Nested)
    }
}

/// Whether a tool may run alongside the other calls in its batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExecutionMode {
    #[default]
    Parallel,
    /// Makes the whole batch run one call at a time.
    Sequential,
    /// Runs alongside the batch's other calls to the same tool, and
    /// apart from every other call. The batch runs in groups, one after
    /// another, in the order their first calls come: each grouped
    /// tool's calls, and the calls to all other tools.
    Grouped,
}

/// Identifies a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct RunId(pub Arc<str>);

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Sends partial output while a tool runs. Updates sent after the tool's
/// future has resolved are ignored.
#[derive(Debug, Clone)]
pub struct ToolUpdates {
    call_id: Arc<str>,
    sender: mpsc::UnboundedSender<(Arc<str>, ToolOutput)>,
    open: Arc<AtomicBool>,
}

impl ToolUpdates {
    pub(crate) fn new(
        call_id: Arc<str>,
        sender: mpsc::UnboundedSender<(Arc<str>, ToolOutput)>,
    ) -> Self {
        Self {
            call_id,
            sender,
            open: Arc::new(AtomicBool::new(true)),
        }
    }

    /// An update sink nobody reads, for calling a tool outside the loop,
    /// as a plugin does with its own tools.
    pub fn detached() -> Self {
        let (sender, _) = mpsc::unbounded_channel();
        Self::new("detached".into(), sender)
    }

    /// An update sink for tests of tools outside the loop.
    pub fn for_tests(
        call_id: &str,
        sender: mpsc::UnboundedSender<(Arc<str>, ToolOutput)>,
    ) -> Self {
        Self::new(call_id.into(), sender)
    }

    /// Reports partial output. Returns false if it was ignored.
    pub fn send(&self, partial: ToolOutput) -> bool {
        self.open.load(Ordering::SeqCst)
            && self.sender.send((self.call_id.clone(), partial)).is_ok()
    }

    /// Stops accepting updates; called when the tool's future resolves.
    pub(crate) fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
    }
}

/// What a running tool gets besides its arguments.
#[derive(Debug, Clone)]
pub struct ToolCtx {
    pub cancel: CancellationToken,
    pub updates: ToolUpdates,
    pub run: RunId,
    /// What a sub-agent started by the tool joins; set by the loop.
    pub(crate) scope: Option<RunScope>,
    /// The run's context of the plugin that added the tool.
    pub(crate) plugin: Option<PluginCtx>,
    /// The way back into the loop for nested calls; set by the loop.
    pub(crate) nesting: Option<Nesting>,
}

/// Why a nested call failed before it reached a tool: the call that
/// made it had ended.
pub const ENDED: &str =
    "Tools can be called only while the tool calling them runs";

/// Why a nested call failed outside a run.
pub const NOT_IN_RUN: &str = "Tools can be called from a tool only in a run";

impl ToolCtx {
    /// A context for calling a tool outside a run, as tests do. A
    /// sub-agent tool called with it fails: it has no run to join. So
    /// does [`Self::call`], and [`Self::catalog`] is empty.
    pub fn new(
        cancel: CancellationToken,
        updates: ToolUpdates,
        run: RunId,
    ) -> Self {
        Self {
            cancel,
            updates,
            run,
            scope: None,
            plugin: None,
            nesting: None,
        }
    }

    /// The run's [`PluginCtx`] of the plugin that added this tool, by
    /// `Plugin::tools`, `RunPlan::add_tool` or its `ToolSource`; `None`
    /// for the agent's own tools and outside a run.
    pub fn plugin(&self) -> Option<&PluginCtx> {
        self.plugin.as_ref()
    }

    /// Every tool this call could call, as the run has them now: the
    /// run's `Direct` and `Nested` tools, then the tool sources' tools
    /// that no run tool's name hides, with the sources' namespaces.
    pub fn catalog(&self) -> Catalog {
        self.nesting
            .as_ref()
            .map_or_else(Catalog::default, |nesting| nesting.tools.catalog())
    }

    /// Calls a tool through the run's loop, as the model's calls go:
    /// lookup, argument repair, validation, every plugin's `before_tool`
    /// and `after_tool_result`, and events whose `parent` is this call's
    /// id. The call's id is `<this call's id>/<n>`, `n` counting from 1.
    ///
    /// The result comes back here and never reaches the transcript. A
    /// failed call (an unknown or `ModelOnly` tool, invalid arguments, a
    /// block, an error from the tool) is `Err`, with the output the
    /// model would have seen in [`ToolError::Output`]. Calls made once
    /// this call has ended fail, and ending it cancels the calls it left
    /// running.
    pub async fn call(
        &self,
        name: &str,
        args: Value,
    ) -> Result<ToolOutput, ToolError> {
        let Some(nesting) = &self.nesting else {
            return Err(NOT_IN_RUN.into());
        };
        if !nesting.open.load(Ordering::SeqCst) {
            return Err(ENDED.into());
        }
        let (reply, answer) = oneshot::channel();
        let request = NestedRequest {
            scope: nesting.scope,
            name: name.to_owned(),
            args,
            reply,
        };
        if nesting.requests.send(request).is_err() {
            return Err(ENDED.into());
        }
        answer.await.unwrap_or_else(|_| Err(ENDED.into()))
    }
}

/// What a call needs to make nested calls: its place in the loop.
#[derive(Clone)]
pub(crate) struct Nesting {
    /// The loop's key for the calls this call makes.
    pub scope: u64,
    /// Closed when this call's future resolves.
    pub open: Arc<AtomicBool>,
    pub requests: mpsc::UnboundedSender<NestedRequest>,
    pub tools: Arc<Toolbox>,
}

impl fmt::Debug for Nesting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Nesting")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// A nested call on its way to the loop.
pub(crate) struct NestedRequest {
    pub scope: u64,
    pub name: String,
    pub args: Value,
    pub reply: oneshot::Sender<Result<ToolOutput, ToolError>>,
}

/// A group of tools, such as one MCP server's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Namespace {
    /// The prefix of its tools' names, such as `mcp__linear`.
    pub name: String,
    pub description: String,
    /// How to use its tools, such as an MCP server's instructions.
    pub instructions: Option<String>,
    /// The names of its tools.
    pub tools: Vec<String>,
}

/// Tools that come and go while runs go on, resolved by name at call
/// time: an MCP server's. A plugin gives one with `Plugin::tool_source`.
///
/// Its tools are never declared to the model, whatever their exposure:
/// tools from tools only. A `ModelOnly` one cannot be called at all. A
/// run's own tools hide source tools of the same name.
#[async_trait]
pub trait ToolSource: Send + Sync + 'static {
    /// The tools it offers now.
    fn tools(&self) -> Vec<Arc<dyn AgentTool>>;

    /// The namespaces its tools are in now.
    fn namespaces(&self) -> Vec<Namespace> {
        Vec::new()
    }

    /// Waits until the named namespaces are ready (an MCP server has
    /// connected and listed its tools, or failed to), or all of them for
    /// `None`, or until `cancel`. Returns at once by default.
    async fn ready(
        &self,
        namespaces: Option<&[String]>,
        cancel: &CancellationToken,
    ) {
        let _ = (namespaces, cancel);
    }
}

/// The tools a tool can call, from [`ToolCtx::catalog`].
#[derive(Clone, Default)]
pub struct Catalog {
    tools: Vec<Arc<dyn AgentTool>>,
    namespaces: Vec<Namespace>,
    sources: Vec<Arc<dyn ToolSource>>,
}

impl Catalog {
    pub(crate) fn new(
        tools: Vec<Arc<dyn AgentTool>>,
        namespaces: Vec<Namespace>,
        sources: Vec<Arc<dyn ToolSource>>,
    ) -> Self {
        Self {
            tools,
            namespaces,
            sources,
        }
    }

    /// The callable tools: the run's first, in the order they were
    /// added, then each source's.
    pub fn tools(&self) -> &[Arc<dyn AgentTool>] {
        &self.tools
    }

    /// The callable tool named `name`.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn AgentTool>> {
        self.tools.iter().find(|tool| tool.name() == name)
    }

    /// The sources' namespaces.
    pub fn namespaces(&self) -> &[Namespace] {
        &self.namespaces
    }

    /// The namespace named `name`.
    pub fn namespace(&self, name: &str) -> Option<&Namespace> {
        self.namespaces.iter().find(|space| space.name == name)
    }

    /// Waits until every source has the named namespaces ready, or all
    /// of its namespaces for `None`. Take a new catalog afterwards to see
    /// the tools they brought.
    pub async fn ready(
        &self,
        namespaces: Option<&[String]>,
        cancel: &CancellationToken,
    ) {
        futures_util::future::join_all(
            self.sources
                .iter()
                .map(|source| source.ready(namespaces, cancel)),
        )
        .await;
    }
}

impl fmt::Debug for Catalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Catalog")
            .field(
                "tools",
                &self
                    .tools
                    .iter()
                    .map(|tool| tool.name())
                    .collect::<Vec<_>>(),
            )
            .field("namespaces", &self.namespaces)
            .finish_non_exhaustive()
    }
}

/// The part of a run that a sub-agent run started by one of its tools
/// shares: the store, the workflow, the event subscriber, and the usage
/// of its children, which counts toward the run's limits. A forking
/// sub-agent also starts from the call, the turn that made it, and what
/// the run had stored when it started.
#[derive(Debug, Clone)]
pub(crate) struct RunScope {
    pub store: Store,
    pub workflow: Option<Arc<str>>,
    pub events: Option<mpsc::Sender<RunEvent>>,
    pub children: Arc<Mutex<Usage>>,
    /// The id of the call being run.
    pub call: Arc<str>,
    /// The assistant message that made the call. The run stores it only
    /// once its tools are done.
    pub turn: Arc<AssistantMessage>,
    /// The `seq` of the run's last stored entry when the call started.
    pub stored: i64,
}

/// A tool, as the loop sees it.
#[async_trait]
pub trait AgentTool: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// The JSON schema of the arguments.
    fn parameters(&self) -> &Value;
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Parallel
    }
    /// Who sees the tool: the model, other tools, or both.
    fn exposure(&self) -> Exposure {
        Exposure::Direct
    }
    /// The JSON schema of [`ToolOutput::structured`], if the tool fills
    /// it.
    fn output_schema(&self) -> Option<&Value> {
        None
    }
    /// Repairs raw arguments before validation, for common model mistakes.
    fn prepare_arguments(&self, raw: Value) -> Value {
        raw
    }
    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError>;
}

/// A tool with typed arguments. Wrap it with [`typed`].
#[async_trait]
pub trait TypedTool: Send + Sync + 'static {
    type Args: DeserializeOwned + JsonSchema + Send;
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    async fn call(
        &self,
        args: Self::Args,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError>;
}

/// An [`AgentTool`] made from a [`TypedTool`].
pub struct TypedAdapter<T: TypedTool> {
    tool: T,
    parameters: Value,
}

/// Turns a [`TypedTool`] into an [`AgentTool`], with its argument schema
/// generated from `T::Args`.
pub fn typed<T: TypedTool>(tool: T) -> TypedAdapter<T> {
    let parameters = serde_json::to_value(schemars::schema_for!(T::Args))
        .expect("a generated schema is valid JSON");
    TypedAdapter { tool, parameters }
}

#[async_trait]
impl<T: TypedTool> AgentTool for TypedAdapter<T> {
    fn name(&self) -> &str {
        T::NAME
    }

    fn description(&self) -> &str {
        T::DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: T::Args = serde_json::from_value(args)?;
        self.tool.call(args, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Updates reach the loop while the tool runs, and are ignored once
    /// it has resolved (`docs/reference/agent-loop.md`, "Updates after
    /// completion").
    #[test]
    fn updates_stop_after_close() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let updates = ToolUpdates::new("call_1".into(), sender);
        assert!(updates.send(ToolOutput::text("half")));
        updates.close();
        assert!(!updates.send(ToolOutput::text("late")));
        let (call, output) = receiver.try_recv().unwrap();
        assert_eq!(&*call, "call_1");
        assert_eq!(output, ToolOutput::text("half"));
        assert!(receiver.try_recv().is_err());
    }

    /// A clone shares the open flag, so closing one closes all.
    #[test]
    fn clones_share_the_open_flag() {
        let (sender, _receiver) = mpsc::unbounded_channel();
        let updates = ToolUpdates::new("call_1".into(), sender);
        let clone = updates.clone();
        updates.close();
        assert!(!clone.send(ToolOutput::text("late")));
    }
}
