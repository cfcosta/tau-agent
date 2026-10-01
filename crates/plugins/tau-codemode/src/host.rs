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

/// The run a script belongs to.
#[async_trait]
pub trait Host: Send + Sync + 'static {
    /// The tools scripts may call. Read once per script.
    fn tools(&self) -> Vec<ToolEntry>;

    /// The namespaces those tools belong to.
    fn namespaces(&self) -> Vec<Namespace> {
        Vec::new()
    }

    /// Runs a tool. `Ok` is the value the script gets: the structured
    /// output when the tool has one (even on an error result that
    /// carries it), else its text as a JSON string. `Err` is the error
    /// text of a failed call with no structured output; the script gets
    /// a Lua error with it.
    ///
    /// The future is dropped when the script ends, times out or is
    /// cancelled before the call finishes.
    async fn call_tool(&self, call: ToolCall) -> Result<Value, String>;

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
