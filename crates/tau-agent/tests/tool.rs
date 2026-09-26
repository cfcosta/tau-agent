//! Tool traits (`tau_agent::tool`).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tau_agent::tool::{
    AgentTool,
    ExecutionMode,
    RunId,
    ToolCtx,
    ToolOutput,
    ToolUpdates,
    TypedTool,
    typed,
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
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::text(format!(
            "{}:{}",
            args.query,
            args.limit.unwrap_or(10)
        )))
    }
}

fn ctx() -> ToolCtx {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        Default::default(),
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

/// A typed tool exposes its name, description and a schema generated
/// from its argument type, and runs with parsed arguments.
#[test]
fn typed_tool_adapts() {
    let tool = typed(Search);
    assert_eq!(tool.name(), "search");
    assert_eq!(tool.description(), "Search the index.");
    assert_eq!(tool.execution_mode(), ExecutionMode::Parallel);
    let schema = tool.parameters();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["query"]["type"], "string");
    assert_eq!(schema["required"], json!(["query"]));
    assert_eq!(
        tool.prepare_arguments(json!({"query": "x"})),
        json!({"query": "x"})
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let output = runtime
        .block_on(tool.call(json!({"query": "tokio", "limit": 3}), ctx()))
        .unwrap();
    assert_eq!(output, ToolOutput::text("tokio:3"));
    // Arguments that do not match the type are an error, not a panic.
    let error = runtime
        .block_on(tool.call(json!({"query": 5}), ctx()))
        .unwrap_err();
    assert!(error.to_string().contains("invalid type"), "{error}");
}

#[test]
fn text_output_has_one_text_block() {
    let output = ToolOutput::text("done");
    assert_eq!(output.details, None);
    assert_eq!(
        serde_json::to_value(&output.content).unwrap(),
        json!([{"type": "text", "text": "done"}])
    );
}

/// A run id displays as the id itself.
#[test]
fn run_ids_display_as_themselves() {
    assert_eq!(RunId("run_1".into()).to_string(), "run_1");
}
