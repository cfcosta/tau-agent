// `before_tool` decides with a `Decision`; returning nothing is an error.
use async_trait::async_trait;
use tau_agent::hook::{HookCtx, RunHook, ToolCall};

struct Quiet;

#[async_trait]
impl RunHook for Quiet {
    async fn before_tool(&self, _call: &mut ToolCall, _ctx: &HookCtx) {}
}

fn main() {}
