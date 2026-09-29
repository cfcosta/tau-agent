//! Hooks: code that runs around tool calls and sees every run event.
//!
//! Hooks run in registration order and are awaited, so a slow hook holds
//! up its own run and no other (`docs/reference/agent-loop.md`).

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    error::PluginError,
    event::RunEvent,
    tool::{RunId, ToolOutput},
};

/// A tool call as hooks see it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// Context for a hook call.
#[derive(Debug, Clone)]
pub struct HookCtx {
    pub run: RunId,
    pub parent: Option<RunId>,
}

/// Whether a tool call may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Refuses the call; the reason becomes the error result the model
    /// sees.
    Block(String),
}

#[async_trait]
pub trait RunHook: Send + Sync + 'static {
    /// Runs before a tool call, after its arguments were validated. May
    /// change the arguments, which are then validated again, or block the
    /// call. An error blocks the call too. The first hook that blocks
    /// wins; later hooks do not run.
    async fn before_tool(
        &self,
        call: &mut ToolCall,
        ctx: &HookCtx,
    ) -> Result<Decision, PluginError> {
        let _ = (call, ctx);
        Ok(Decision::Allow)
    }

    /// Runs after a tool call, and may change its output.
    async fn after_tool(
        &self,
        call: &ToolCall,
        output: &mut ToolOutput,
        ctx: &HookCtx,
    ) {
        let _ = (call, output, ctx);
    }

    /// Sees every run event, in order.
    async fn on_event(&self, event: &RunEvent) {
        let _ = event;
    }
}
