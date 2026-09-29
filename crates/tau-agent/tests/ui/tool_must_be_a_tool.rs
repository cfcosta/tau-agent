// Only tools can be added as tools: a typed tool must be wrapped with
// `typed`.
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tau_agent::{
    agent::Agent,
    error::ToolError,
    tool::{ToolCtx, ToolOutput, TypedTool},
};
use tau_testing::scripted::ScriptedModel;

#[derive(Deserialize, JsonSchema)]
struct Args {
    query: String,
}

struct Search;

#[async_trait]
impl TypedTool for Search {
    type Args = Args;
    const NAME: &'static str = "search";
    const DESCRIPTION: &'static str = "Search.";

    async fn call(
        &self,
        args: Args,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(args.query))
    }
}

fn main() {
    let _ = Agent::new(ScriptedModel::new()).tool(Search);
}
