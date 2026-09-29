//! `write`: create or overwrite a file (`docs/reference/tools.md`,
//! "write"), ported from pi's `write.ts`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};

use crate::{ABORTED, lock, path::Root};

const DESCRIPTION: &str = "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.";

/// Arguments of the `write` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteArgs {
    /// Path to the file to write (relative or absolute)
    pub path: String,
    /// Content to write to the file
    pub content: String,
}

/// The `write` tool: create or overwrite a file, under the per-path
/// lock shared with `edit`.
pub struct Write {
    root: Root,
    parameters: Value,
}

impl Write {
    pub fn new(root: Root) -> Self {
        let parameters = serde_json::to_value(schemars::schema_for!(WriteArgs))
            .expect("a generated schema is valid JSON");
        Self { root, parameters }
    }
}

#[async_trait]
impl AgentTool for Write {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: WriteArgs = serde_json::from_value(args)?;
        let resolved = self.root.resolve(&args.path);
        let _guard = lock::lock(&resolved).await;

        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }

        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }

        tokio::fs::write(&resolved, args.content.as_bytes()).await?;
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }

        Ok(ToolOutput::text(format!(
            "Successfully wrote to {}",
            args.path
        )))
    }
}
