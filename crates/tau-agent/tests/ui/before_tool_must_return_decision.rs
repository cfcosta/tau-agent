// `before_tool` decides with a `Decision`; returning nothing is an error.
use async_trait::async_trait;
use tau_agent::plugin::{PluginCtx, PluginRun, ToolCall};

struct Quiet;

#[async_trait]
impl PluginRun for Quiet {
    async fn before_tool(&mut self, _call: &mut ToolCall, _ctx: &PluginCtx) {}
}

fn main() {}
