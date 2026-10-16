//! Tool traits (`tau_agent::tool`).

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput, TypedTool, typed},
};

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchArgs {
    /// What to look for.
    query: String,
    limit: Option<u32>,
}

struct Search;

#[async_trait]
impl TypedTool for Search {
    type Args = SearchArgs;
    const NAME: &'static str = "search";
    const DESCRIPTION: &'static str = "Search the index.";

    async fn call(
        &self,
        args: SearchArgs,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(format!(
            "{}:{}",
            args.query,
            args.limit.unwrap_or(10)
        )))
    }
}

/// A typed tool's schema is generated from its argument type (an
/// `Option` field optional, the rest required), and it runs with the
/// arguments parsed into that type.
#[test]
fn typed_tool_adapts() {
    let tool = typed(Search);
    let schema = tool.parameters();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["query"]["type"], "string");
    assert_eq!(schema["required"], json!(["query"]));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let output =
        runtime
            .block_on(tool.call(
                json!({"query": "tokio", "limit": 3}),
                ToolCtx::detached(),
            ))
            .unwrap();
    assert_eq!(output, ToolOutput::text("tokio:3"));
    // Arguments that do not match the type are an error, not a panic.
    assert!(
        runtime
            .block_on(tool.call(json!({"query": 5}), ToolCtx::detached()))
            .is_err()
    );
}
