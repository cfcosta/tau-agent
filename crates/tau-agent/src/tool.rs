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
use tau_ai::message::{InputBlock, TextContent, Usage};
use tau_store::Store;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::event::RunEvent;

/// What a tool returns.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolOutput {
    /// What the model sees.
    pub content: Vec<InputBlock>,
    /// Tool-specific details for callers and hooks; the model never sees
    /// them.
    pub details: Option<Value>,
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
        }
    }
}

/// Whether a tool may run alongside the other calls in its batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExecutionMode {
    #[default]
    Parallel,
    /// Makes the whole batch run one call at a time.
    Sequential,
}

/// Identifies a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
}

impl ToolCtx {
    /// A context for calling a tool outside a run, as tests do. A
    /// sub-agent tool called with it fails: it has no run to join.
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
        }
    }
}

/// The part of a run that a sub-agent run started by one of its tools
/// shares: the store, the workflow, the event subscriber, and the usage
/// of its children, which counts toward the run's limits.
#[derive(Debug, Clone)]
pub(crate) struct RunScope {
    pub store: Store,
    pub workflow: Option<Arc<str>>,
    pub events: Option<mpsc::Sender<RunEvent>>,
    pub children: Arc<Mutex<Usage>>,
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
    /// Repairs raw arguments before validation, for common model mistakes.
    fn prepare_arguments(&self, raw: Value) -> Value {
        raw
    }
    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> anyhow::Result<ToolOutput>;
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
    ) -> anyhow::Result<ToolOutput>;
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
    ) -> anyhow::Result<ToolOutput> {
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
