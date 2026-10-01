//! `ls`: list directory contents (`docs/reference/tools.md`, "ls"),
//! ported from pi's `ls.ts`. The model reads pi's names; callers get a
//! [`Listing`] in the details, with each entry's kind, size, age and
//! whether `.gitignore` leaves it out.

use std::{collections::HashSet, fs::Metadata, path::Path, time::UNIX_EPOCH};

use async_trait::async_trait;
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
    details::{Entry, EntryKind, Listing},
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
    ) -> Result<ToolOutput, ToolError> {
        let args: LsArgs = serde_json::from_value(args)?;
        let root = self.root.clone();
        let cancel = ctx.cancel.clone();
        let (text, listing) =
            tokio::task::spawn_blocking(move || run(&root, args, &cancel))
                .await??;
        let mut output = ToolOutput::text(text);
        output.details = listing.map(serde_json::to_value).transpose()?;
        Ok(output)
    }
}

fn run(
    root: &Root,
    args: LsArgs,
    cancel: &CancellationToken,
) -> Result<(String, Option<Listing>), ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::from(ABORTED));
    }
    let dir_path = root.resolve(args.path.as_deref().unwrap_or("."));
    if !dir_path.exists() {
        return Err(ToolError::from(format!(
            "Path not found: {}",
            dir_path.display()
        )));
    }
    if !dir_path.is_dir() {
        return Err(ToolError::from(format!(
            "Not a directory: {}",
            dir_path.display()
        )));
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT) as usize;

    let mut names: Vec<String> = match std::fs::read_dir(&dir_path) {
        Ok(read_dir) => read_dir
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(err) => {
            return Err(ToolError::from(format!(
                "Cannot read directory: {err}"
            )));
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

    let kept = unignored(&dir_path);
    let mut results: Vec<String> = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    let mut limit_reached = false;
    for name in &names {
        if cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
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
        entries.push(entry(name, &full, &meta, &kept));
    }

    if results.is_empty() {
        return Ok(("(empty directory)".to_owned(), None));
    }

    let raw = results.join("\n");
    let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
    let truncated = truncation.truncated();
    let mut text = truncation.content;
    // The byte cap cuts whole lines: the model saw as many entries
    // as it got lines.
    entries.truncate(text.lines().count());

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
    let listing = Listing {
        dir: dir_path.to_string_lossy().into_owned(),
        entries,
        truncated: limit_reached || truncated,
    };
    Ok((text, Some(listing)))
}

/// What the listing says of `name`. `meta` follows symlinks, as the
/// model's `/` does.
fn entry(
    name: &str,
    full: &Path,
    meta: &Metadata,
    kept: &Option<HashSet<String>>,
) -> Entry {
    let target = std::fs::symlink_metadata(full)
        .is_ok_and(|link| link.file_type().is_symlink())
        .then(|| std::fs::read_link(full).ok())
        .flatten()
        .map(|target| target.to_string_lossy().into_owned());
    let kind = match (&target, meta.is_dir()) {
        (Some(_), true) => EntryKind::SymlinkDir,
        (Some(_), false) => EntryKind::Symlink,
        (None, true) => EntryKind::Dir,
        (None, false) => EntryKind::File,
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|age| i64::try_from(age.as_secs()).ok());
    let items = meta
        .is_dir()
        .then(|| std::fs::read_dir(full).ok())
        .flatten()
        .map(|read_dir| read_dir.count() as u64);
    Entry {
        name: name.to_owned(),
        kind,
        size: meta.is_file().then_some(meta.len()),
        modified,
        items,
        target,
        ignored: kept.as_ref().is_some_and(|kept| !kept.contains(name)),
    }
}

/// The names in `dir` that `.gitignore` keeps, with the rules `find`
/// and `grep` walk by. `None` when the walk fails, so nothing reads as
/// ignored.
fn unignored(dir: &Path) -> Option<HashSet<String>> {
    let mut walk = WalkBuilder::new(dir);
    walk.hidden(false)
        .require_git(false)
        .git_global(false)
        .git_exclude(false)
        .max_depth(Some(1));
    let mut kept = HashSet::new();
    for result in walk.build() {
        let entry = result.ok()?;
        if entry.depth() == 1 {
            kept.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Some(kept)
}
