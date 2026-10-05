//! What a script reaches outside its VM: the [`Host`].
//!
//! The plugin implements it over `ToolCtx::call` and the run's tools;
//! tests implement it with fakes. Every global that reaches out of the
//! VM goes through it.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tau_ai::message::Usage;
use tau_jev::Jev;

use crate::modules::Definition;

/// A callable tool, as scripts see it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolEntry {
    /// Its key in `tools`: already a Luau identifier.
    pub name: String,
    pub description: String,
    /// The JSON Schema of its arguments.
    pub input_schema: Value,
    /// The JSON Schema of its structured output, if it has one.
    pub output_schema: Option<Value>,
    /// The namespace it belongs to, such as `mcp__linear`.
    pub namespace: Option<String>,
    /// Its calls run one at a time, as the loop runs a `Sequential`
    /// tool.
    pub sequential: bool,
}

/// A group of tools, such as an MCP server's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Namespace {
    pub name: String,
    pub description: Option<String>,
    /// How to use its tools (an MCP server's `instructions`).
    pub instructions: Option<String>,
}

/// One nested tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// `<parent>/<n>`, numbered from 1 in the order calls start.
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// A readable nested-call value and its independent execution status.
/// A structured error is returned to the script without raising, but
/// `error: Some(..)` keeps its call row failed, including an empty message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolReply {
    pub value: Value,
    pub error: Option<String>,
    /// Authoritative usage from the reserved inference tool. The engine
    /// includes it in display totals; the agent loop charged it already.
    pub usage: Option<Usage>,
    /// Whether every admitted inference attempt has final SDK usage.
    pub usage_complete: Option<bool>,
}

impl ToolReply {
    pub fn success(value: Value) -> Self {
        Self {
            value,
            error: None,
            usage: None,
            usage_complete: None,
        }
    }
}

/// The run a script belongs to.
#[async_trait]
pub trait Host: Send + Sync + 'static {
    /// Maximum bytes retained from script output. Tests use a smaller bound.
    fn output_byte_limit(&self) -> usize {
        crate::MAX_OUTPUT_BYTES
    }

    /// The tools scripts may call. Read once per script.
    fn tools(&self) -> Vec<ToolEntry>;

    /// The namespaces those tools belong to.
    fn namespaces(&self) -> Vec<Namespace> {
        Vec::new()
    }

    /// Finds a registered module by selected version or exact digest.
    /// The default host has no module library.
    async fn module(
        &self,
        _name: &str,
        _version: Option<&str>,
    ) -> Result<Option<Definition>, String> {
        Ok(None)
    }

    /// Runs a tool. `Ok` carries the readable value: structured output
    /// when the tool has one, else successful text as a JSON string.
    /// A failed call with structured output returns `Ok` with `error`
    /// set; its value remains readable while its call row stays failed.
    /// `Err` is a failure without a readable value and raises a Lua error.
    ///
    /// The future is dropped when the script ends, times out or is
    /// cancelled before the call finishes.
    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String>;

    /// The Jev the `jev` global asks, if the run has one.
    fn jev(&self) -> Option<Arc<dyn Jev>> {
        None
    }

    /// Charges one Jev request's usage to the run.
    fn charge(&self, _usage: &Usage) {}

    /// Reports what the script is doing while it runs, as the details
    /// of a partial result ([`crate::live`]): the plugin sends them as
    /// the codemode call's `ToolUpdate`s.
    fn update(&self, _details: Value) {}
}
