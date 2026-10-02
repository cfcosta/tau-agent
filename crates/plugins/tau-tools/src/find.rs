//! `find`: search for files by glob pattern (`docs/reference/tools.md`,
//! "find"), ported from pi's `find.ts`.
//!
//! Search uses `globset` together with `ignore::WalkBuilder` (hidden
//! files included, `.gitignore` respected). There is no `fd`
//! subprocess.
//!
//! **Deliberate difference from pi:** the "limit reached" notice
//! appears only when there really were more results; pi shows it
//! whenever the count equals the limit (`find.ts:145`).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tokio_util::sync::CancellationToken;

use crate::{
    ABORTED,
    SearchError,
    path::Root,
    truncate::{MAX_BYTES, format_size, truncate_head},
};

const DEFAULT_LIMIT: u32 = 1000;

const DESCRIPTION: &str = "Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to 1000 results or 50KB (whichever is hit first).";

/// `find`'s arguments, pi's field names (`find.ts`).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindArgs {
    /// Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'
    pub pattern: String,
    /// Directory to search in (default: current directory)
    pub path: Option<String>,
    /// Maximum number of results (default: 1000)
    pub limit: Option<u32>,
}

/// Program-visible result from `find`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FindOutput {
    /// The resolved path searched by this call.
    pub root: String,
    /// Matched paths relative to `root`, in the tool's existing order.
    pub paths: Vec<String>,
    /// Whether the entry or display-byte limit omitted part of the result.
    pub truncated: bool,
    /// Whether the walk and both result limits returned a complete result.
    pub complete: bool,
    /// Whether another matching path existed beyond the entry limit.
    pub limit_reached: bool,
    /// Whether the formatted text exceeded its byte limit.
    pub bytes_truncated: bool,
    /// Number of filesystem entries the walker could not read.
    pub skipped: usize,
}

struct FindResult {
    text: String,
    output: FindOutput,
}

/// Finds files by glob pattern (`docs/reference/tools.md`, "find").
pub struct Find {
    root: Root,
    parameters_schema: Value,
    output_schema: Value,
}

impl Find {
    pub fn new(root: Root) -> Self {
        let parameters_schema =
            serde_json::to_value(schemars::schema_for!(FindArgs))
                .expect("a generated schema is valid JSON");
        let output_schema =
            serde_json::to_value(schemars::schema_for!(FindOutput))
                .expect("a generated schema is valid JSON");
        Self {
            root,
            parameters_schema,
            output_schema,
        }
    }
}

#[async_trait]
impl AgentTool for Find {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.parameters_schema
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output_schema)
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: FindArgs = serde_json::from_value(args)?;
        let root = self.root.clone();
        let cancel = ctx.cancel.clone();
        let result =
            tokio::task::spawn_blocking(move || run(&root, args, &cancel))
                .await??;
        let mut output = ToolOutput::text(result.text);
        output.structured = Some(
            serde_json::to_value(result.output)
                .expect("FindOutput serializes to valid JSON"),
        );
        Ok(output)
    }
}

/// Whether `pattern` matches against the full relative path (it
/// contains a `/`, with an implicit `**/` prefix unless it already
/// anchors itself) or just a file's name, and the pattern to compile
/// (pi, `find.ts`'s `effectivePattern`).
fn effective_pattern(pattern: &str) -> (bool, String) {
    if !pattern.contains('/') {
        return (false, pattern.to_owned());
    }
    // pi also leaves "**" alone, but it has no `/` and never gets here.
    let full = if pattern.starts_with('/') || pattern.starts_with("**/") {
        pattern.to_owned()
    } else {
        format!("**/{pattern}")
    };
    (true, full)
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn relative_posix(search_path: &Path, file: &Path) -> String {
    file.strip_prefix(search_path)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

fn walk_files(
    search_path: &Path,
    cancel: &CancellationToken,
) -> Result<(Vec<PathBuf>, usize), ToolError> {
    let mut files = Vec::new();
    let mut skipped = 0;
    // `.gitignore` applies whether or not the tree sits inside an
    // actual git repository (`ignore`'s default requires one). The
    // user's personal global gitignore and `.git/info/exclude` are
    // not part of the spec and would otherwise leak in once
    // `require_git` no longer gates them.
    let mut walk = WalkBuilder::new(search_path);
    walk.hidden(false)
        .require_git(false)
        .git_global(false)
        .git_exclude(false);
    for result in walk.build() {
        if cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }
        let Ok(entry) = result else {
            skipped += 1;
            continue;
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        files.push(entry.into_path());
    }
    files.sort();
    Ok((files, skipped))
}

/// `pattern` as a glob whose `*` stops at a `/`.
fn glob(pattern: &str) -> Result<GlobMatcher, SearchError> {
    Ok(GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()?
        .compile_matcher())
}

fn run(
    root: &Root,
    args: FindArgs,
    cancel: &CancellationToken,
) -> Result<FindResult, ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::from(ABORTED));
    }
    let search_path = root.resolve(args.path.as_deref().unwrap_or("."));
    if !search_path.exists() {
        return Err(ToolError::from(format!(
            "Path not found: {}",
            search_path.display()
        )));
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT) as usize;
    let (full_path_mode, pattern) = effective_pattern(&args.pattern);
    let glob = glob(&pattern)?;

    let is_dir = search_path.is_dir();
    let (files, skipped) = if is_dir {
        walk_files(&search_path, cancel)?
    } else {
        (vec![search_path.clone()], 0)
    };

    let mut matched: Vec<String> = Vec::new();
    let mut limit_reached = false;
    for file in &files {
        if cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }
        let candidate = if full_path_mode {
            relative_posix(&search_path, file)
        } else {
            basename(file)
        };
        if !glob.is_match(&candidate) {
            continue;
        }
        if matched.len() >= limit {
            limit_reached = true;
            break;
        }
        let rel = if is_dir {
            relative_posix(&search_path, file)
        } else {
            basename(file)
        };
        matched.push(rel);
    }

    let raw = matched.join("\n");
    let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
    let bytes_truncated = truncation.truncated();
    let text = if matched.is_empty() && !limit_reached {
        "No files found matching pattern".to_owned()
    } else {
        let mut text = truncation.content;

        let mut notices = Vec::new();
        if limit_reached {
            notices.push(format!(
                "{limit} results limit reached. Use limit={} for more, or refine pattern",
                limit * 2
            ));
        }
        if bytes_truncated {
            notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }
        text
    };

    // Preserve the selected path records before display truncation. Text
    // lines are ambiguous when a valid path itself contains a newline.
    let truncated = limit_reached || bytes_truncated;
    let complete = !truncated && skipped == 0;
    Ok(FindResult {
        text,
        output: FindOutput {
            root: search_path.to_string_lossy().into_owned(),
            paths: matched,
            truncated,
            complete,
            limit_reached,
            bytes_truncated,
            skipped,
        },
    })
}
