//! `ls`: list directory contents (`docs/reference/tools.md`, "ls"),
//! ported from pi's `ls.ts`.

use anyhow::anyhow;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tau_agent::tool::{AgentTool, ToolCtx, ToolOutput};
use tokio_util::sync::CancellationToken;

use crate::{
    ABORTED,
    path::Root,
    truncate::{MAX_BYTES, format_size, truncate_head},
};

const DEFAULT_LIMIT: u32 = 500;

const DESCRIPTION: &str = "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to 500 entries or 50KB (whichever is hit first).";

/// `ls`'s arguments, pi's field names (`ls.ts`).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LsArgs {
    /// Directory to list (default: current directory)
    pub path: Option<String>,
    /// Maximum number of entries to return (default: 500)
    pub limit: Option<u32>,
}

/// Lists a directory's contents (`docs/reference/tools.md`, "ls").
pub struct Ls {
    root: Root,
    schema: Value,
}

impl Ls {
    pub fn new(root: Root) -> Self {
        let schema = serde_json::to_value(schemars::schema_for!(LsArgs))
            .expect("a generated schema is valid JSON");
        Self { root, schema }
    }
}

#[async_trait]
impl AgentTool for Ls {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.schema
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> anyhow::Result<ToolOutput> {
        let args: LsArgs = serde_json::from_value(args)?;
        let root = self.root.clone();
        let cancel = ctx.cancel.clone();
        let text =
            tokio::task::spawn_blocking(move || run(&root, args, &cancel))
                .await??;
        Ok(ToolOutput::text(text))
    }
}

fn run(
    root: &Root,
    args: LsArgs,
    cancel: &CancellationToken,
) -> anyhow::Result<String> {
    if cancel.is_cancelled() {
        return Err(anyhow!(ABORTED));
    }
    let dir_path = root.resolve(args.path.as_deref().unwrap_or("."));
    if !dir_path.exists() {
        return Err(anyhow!("Path not found: {}", dir_path.display()));
    }
    if !dir_path.is_dir() {
        return Err(anyhow!("Not a directory: {}", dir_path.display()));
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT) as usize;

    let mut names: Vec<String> = match std::fs::read_dir(&dir_path) {
        Ok(read_dir) => read_dir
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(err) => {
            return Err(anyhow!("Cannot read directory: {err}"));
        }
    };
    // Case-insensitively, and names equal but for case in byte order,
    // as pi gets them: libuv's `scandir` returns names in `strcmp`
    // order, which pi's stable sort keeps for ties. `read_dir`'s own
    // order is the filesystem's, so ties need an explicit order.
    names.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });

    let mut results: Vec<String> = Vec::new();
    let mut limit_reached = false;
    for name in &names {
        if cancel.is_cancelled() {
            return Err(anyhow!(ABORTED));
        }
        if results.len() >= limit {
            limit_reached = true;
            break;
        }
        let full = dir_path.join(name);
        let Ok(meta) = std::fs::metadata(&full) else {
            continue;
        };
        let suffix = if meta.is_dir() { "/" } else { "" };
        results.push(format!("{name}{suffix}"));
    }

    if results.is_empty() {
        return Ok("(empty directory)".to_owned());
    }

    let raw = results.join("\n");
    let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
    let truncated = truncation.truncated();
    let mut text = truncation.content;

    let mut notices = Vec::new();
    if limit_reached {
        notices.push(format!(
            "{limit} entries limit reached. Use limit={} for more",
            limit * 2
        ));
    }
    if truncated {
        notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
    }
    if !notices.is_empty() {
        text.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }
    Ok(text)
}
