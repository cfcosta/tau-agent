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
use serde::Deserialize;
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

/// Finds files by glob pattern (`docs/reference/tools.md`, "find").
pub struct Find {
    root: Root,
    schema: Value,
}

impl Find {
    pub fn new(root: Root) -> Self {
        let schema = serde_json::to_value(schemars::schema_for!(FindArgs))
            .expect("a generated schema is valid JSON");
        Self { root, schema }
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
        &self.schema
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: FindArgs = serde_json::from_value(args)?;
        let root = self.root.clone();
        let cancel = ctx.cancel.clone();
        let text =
            tokio::task::spawn_blocking(move || run(&root, args, &cancel))
                .await??;
        Ok(ToolOutput::text(text))
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
) -> Result<Vec<PathBuf>, ToolError> {
    let mut files = Vec::new();
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
        let Ok(entry) = result else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        files.push(entry.into_path());
    }
    files.sort();
    Ok(files)
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
) -> Result<String, ToolError> {
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
    let files = if is_dir {
        walk_files(&search_path, cancel)?
    } else {
        vec![search_path.clone()]
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

    if matched.is_empty() && !limit_reached {
        return Ok("No files found matching pattern".to_owned());
    }

    let raw = matched.join("\n");
    let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
    let truncated = truncation.truncated();
    let mut text = truncation.content;

    let mut notices = Vec::new();
    if limit_reached {
        notices.push(format!(
            "{limit} results limit reached. Use limit={} for more, or refine pattern",
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
