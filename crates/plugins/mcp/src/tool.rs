//! One MCP tool as a tau tool (`docs/reference/mcp.md`, "Definitions").

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    output::Spill,
    tool::{AgentTool, Exposure, ToolCtx, ToolError, ToolOutput},
};

use crate::{
    connection::{Annotations, CallFailure, Connection, Progress, ToolInfo},
    results::map_result,
};

/// The most bytes of a result's `structuredContent`, as JSON, a call's
/// `details` keep for its card: about as much text as the model gets. Past it,
/// the card shows the text; scripts get the whole value either way.
pub const DETAILS_STRUCTURED_LIMIT: usize = 20 * 1024;

/// An MCP server's tool. Its name is `mcp__<server>__<tool>`
/// ([`crate::names`]); calls go through the server's [`Connection`],
/// which connects again when it dropped and never sends a call twice.
///
/// The tool's MCP hints are kept in [`McpTool::annotations`], and every
/// call's `details` carry them too, as
/// `{ "server", "tool", "annotations": { "readOnlyHint", ... },
/// "structuredContent"? }`, for the constitution and the interface.
#[derive(Clone)]
pub struct McpTool {
    name: String,
    info: ToolInfo,
    connection: Arc<Connection>,
    description: String,
    parameters: Value,
    output_schema: Value,
    exposure: Exposure,
    spill: Spill,
}

impl McpTool {
    pub(crate) fn new(
        name: String,
        connection: Arc<Connection>,
        info: ToolInfo,
        exposure: Exposure,
        spill: Spill,
    ) -> Self {
        Self {
            description: description(connection.name(), &info),
            parameters: parameters(&info.input_schema),
            output_schema: output_schema(info.output_schema.as_ref()),
            name,
            info,
            connection,
            exposure,
            spill,
        }
    }

    /// The same tool with another exposure.
    pub(crate) fn exposed(&self, exposure: Exposure) -> Self {
        Self {
            exposure,
            ..self.clone()
        }
    }

    /// The connection its calls go through.
    pub(crate) fn connection(&self) -> &Arc<Connection> {
        &self.connection
    }

    /// The exposure its server's entry gives it.
    pub fn configured_exposure(&self) -> crate::config::Exposure {
        self.connection.config().exposure_of(&self.info.name)
    }

    /// The server's name.
    pub fn server(&self) -> &str {
        self.connection.name()
    }

    /// The tool as the server lists it.
    pub fn info(&self) -> &ToolInfo {
        &self.info
    }

    /// The tool's MCP hints: read-only, destructive, idempotent, open
    /// world.
    pub fn annotations(&self) -> &Annotations {
        &self.info.annotations
    }

    /// What every call's `details` carry: the server, the tool, its
    /// hints, and the result's `structuredContent` when it has one that
    /// fits in [`DETAILS_STRUCTURED_LIMIT`], for the call's card.
    fn details(&self, structured: Option<&Value>) -> Value {
        let mut details = json!({
            "server": self.server(),
            "tool": self.info.name,
            "annotations": self.info.annotations,
        });
        if let Some(content) = structured
            .and_then(|result| result.get("structuredContent"))
            .filter(|content| {
                content.to_string().len() <= DETAILS_STRUCTURED_LIMIT
            })
        {
            details["structuredContent"] = content.clone();
        }
        details
    }
}

/// The tool's description, else its title, else
/// `MCP tool <tool> from server <server>`. Never the server's
/// instructions.
pub fn description(server: &str, info: &ToolInfo) -> String {
    info.description
        .as_deref()
        .or(info.title.as_deref())
        .filter(|text| !text.trim().is_empty())
        .map_or_else(
            || format!("MCP tool {} from server {server}", info.name),
            str::to_owned,
        )
}

/// The tool's `inputSchema`, with `type: "object"` and
/// `properties: {}` added when missing.
pub fn parameters(input_schema: &Value) -> Value {
    let mut schema = match input_schema {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    schema.entry("type").or_insert_with(|| json!("object"));
    schema.entry("properties").or_insert_with(|| json!({}));
    Value::Object(schema)
}

/// The schema of [`ToolOutput::structured`]: a `CallToolResult`,
/// `{ content, structuredContent?, isError }`, with the tool's
/// `outputSchema` as `structuredContent`'s when it has one.
pub fn output_schema(tool_output: Option<&Value>) -> Value {
    let mut properties = json!({
        "content": {"type": "array", "items": {"type": "object"}},
        "isError": {"type": "boolean"},
    });
    if let Some(schema) = tool_output {
        properties["structuredContent"] = schema.clone();
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": ["content", "isError"],
    })
}

/// What a progress notification shows while the call runs.
fn progress_output(progress: &Progress) -> ToolOutput {
    let count = match progress.total {
        Some(total) => format!("{}/{}", progress.progress, total),
        None => progress.progress.to_string(),
    };
    let text = match &progress.message {
        Some(message) => format!("{message} ({count})"),
        None => format!("Progress: {count}"),
    };
    ToolOutput {
        details: Some(json!({
            "progress": progress.progress,
            "total": progress.total,
            "message": progress.message,
        })),
        ..ToolOutput::text(text)
    }
}

#[async_trait]
impl AgentTool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn exposure(&self) -> Exposure {
        self.exposure
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output_schema)
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let updates = ctx.updates.clone();
        let on_progress = move |progress: Progress| {
            updates.send(progress_output(&progress));
        };
        let result = self
            .connection
            .call(&self.info.name, args, &on_progress, &ctx.cancel)
            .await;
        match result {
            Ok(result) => {
                let mapped = map_result(
                    self.server(),
                    &self.info.name,
                    &result,
                    &self.spill,
                );
                let mut output = mapped.output;
                output.details = Some(self.details(output.structured.as_ref()));
                if mapped.is_error {
                    Err(ToolError::output(output))
                } else {
                    Ok(output)
                }
            }
            Err(CallFailure::Withdrawn { server, .. }) => Err(format!(
                "Tool {} is no longer offered by server {server}",
                self.name
            )
            .into()),
            Err(failure) => Err(failure.to_string().into()),
        }
    }
}
