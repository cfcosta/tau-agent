// A typed tool's arguments must describe themselves with a JSON schema.
use async_trait::async_trait;
use serde::Deserialize;
use tau_agent::{
    error::ToolError,
    tool::{ToolCtx, ToolOutput, TypedTool},
};

#[derive(Deserialize)]
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

fn main() {}
