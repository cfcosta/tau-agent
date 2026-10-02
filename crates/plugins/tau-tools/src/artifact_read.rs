//! Authorized, bounded artifact ranges for nested Code Mode calls.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, Exposure, ToolCtx, ToolOutput},
};
use tau_artifacts::{Artifact, Bytes, DEFAULT_RANGE_BYTES, Encoding};

use crate::{ABORTED, artifact_grant::fold_grants};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Args {
    id: String,
    #[serde(default)]
    offset: u64,
    limit: Option<u32>,
    encoding: Option<ReadEncoding>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ReadEncoding {
    Utf8,
    Base64,
}

impl From<ReadEncoding> for Encoding {
    fn from(value: ReadEncoding) -> Self {
        match value {
            ReadEncoding::Utf8 => Self::Utf8,
            ReadEncoding::Base64 => Self::Base64,
        }
    }
}

pub struct ArtifactRead {
    bytes: Option<Bytes>,
    parameters: Value,
    output: Value,
}

impl ArtifactRead {
    pub fn new(bytes: Option<Bytes>) -> Self {
        Self {
            bytes,
            parameters: serde_json::to_value(schemars::schema_for!(Args))
                .unwrap(),
            output: json!({
                "type": "object",
                "required": ["id", "offset", "next_offset", "size_bytes", "encoding", "data", "eof", "complete"],
                "properties": {
                    "id": {"type": "string"},
                    "offset": {"type": "integer"},
                    "next_offset": {"type": ["integer", "null"]},
                    "size_bytes": {"type": "integer"},
                    "encoding": {"enum": ["utf8", "base64"]},
                    "data": {"type": "string"},
                    "eof": {"type": "boolean"},
                    "complete": {"type": "boolean"}
                }
            }),
        }
    }
}

#[async_trait]
impl AgentTool for ArtifactRead {
    fn name(&self) -> &str {
        "artifact_read"
    }
    fn description(&self) -> &str {
        "Read a granted artifact by ID and byte range. UTF-8 pages end at codepoint boundaries; use base64 for binary bytes."
    }
    fn parameters(&self) -> &Value {
        &self.parameters
    }
    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output)
    }
    fn exposure(&self) -> Exposure {
        Exposure::Nested
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        if ctx.cancel.is_cancelled() {
            return Err(ABORTED.into());
        }
        let plugin = ctx
            .plugin()
            .ok_or("artifact_read requires a tool plugin context")?;
        let bytes = self
            .bytes
            .clone()
            .ok_or("artifact storage is unavailable")?;
        let records = plugin.records().await.map_err(ToolError::other)?;
        let grants = fold_grants(&records).map_err(ToolError::from)?;
        let grant = grants
            .get(&args.id)
            .ok_or("artifact is not granted to this run")?;
        // Deserialize only the stored metadata. Caller arguments cannot supply it.
        let artifact: Artifact =
            serde_json::from_value(serde_json::to_value(&grant.artifact)?)?;
        let offset = args.offset;
        let limit = args.limit.unwrap_or(DEFAULT_RANGE_BYTES);
        let encoding = args.encoding.map_or(Encoding::Utf8, Into::into);
        let cancel = ctx.cancel.clone();
        let work = tokio::task::spawn_blocking(move || {
            bytes.read_range(&artifact, offset, limit, encoding, &cancel)
        });
        let range = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(ABORTED.into()),
            result = work => result?.map_err(ToolError::other)?,
        };
        let mut output = ToolOutput::text(format!(
            "Read artifact {} at bytes {}..{}",
            range.id,
            range.offset,
            range.next_offset.unwrap_or(range.size_bytes)
        ));
        output.structured = Some(serde_json::to_value(range)?);
        Ok(output)
    }
}
