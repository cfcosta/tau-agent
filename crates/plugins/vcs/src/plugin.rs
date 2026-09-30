//! The version-control tools as a plugin (`docs/reference/plugins.md`),
//! for `Agent::plugin`.
//!
//! The plugin adds the tools, and marks what `@` changes in each `ls`
//! listing (`docs/reference/vcs.md`, "ls"). It keeps no state per run.
//! Every run of the agent works in the one workspace the [`Vcs`] opened.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use tau_agent::{
    hook::ToolCall,
    plugin::{Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
    tool::{AgentTool, ToolOutput},
};

use crate::{
    diff::{ChangeKind, FileChange},
    tools::{
        Commit,
        Describe,
        Diff,
        Land,
        Log,
        New,
        Restore,
        Show,
        Status,
        Undo,
        tool,
    },
    vcs::Vcs,
};

/// The coding tools' `ls` (`tau_tools::ls`), whose listings the plugin
/// marks.
const LS: &str = "ls";

/// The version-control tools on one workspace, as a plugin. All nine by
/// default; [`read_only`](Self::read_only) keeps the four that change
/// nothing but snapshots.
#[derive(Debug, Clone)]
pub struct VcsPlugin {
    vcs: Vcs,
    write: bool,
    landing: bool,
}

impl VcsPlugin {
    pub fn new(vcs: Vcs) -> Self {
        Self {
            vcs,
            write: true,
            landing: false,
        }
    }

    /// Adds `vcs_land`, for a run that lands on a parent or merges into
    /// trunk when it finishes (ADR 0014).
    pub fn landing(mut self) -> Self {
        self.landing = true;
        self
    }

    /// Keeps only `vcs_status`, `vcs_diff`, `vcs_log` and `vcs_show`.
    pub fn read_only(mut self) -> Self {
        self.write = false;
        self
    }

    pub fn vcs(&self) -> &Vcs {
        &self.vcs
    }
}

#[async_trait]
impl Plugin for VcsPlugin {
    fn name(&self) -> &str {
        "vcs"
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let vcs = &self.vcs;
        let mut tools = vec![
            tool(Status(vcs.clone())),
            tool(Diff(vcs.clone())),
            tool(Log(vcs.clone())),
            tool(Show(vcs.clone())),
        ];
        if self.write {
            tools.extend([
                tool(Describe(vcs.clone())),
                tool(Commit(vcs.clone())),
                tool(New(vcs.clone())),
                tool(Restore(vcs.clone())),
                tool(Undo(vcs.clone())),
            ]);
            if self.landing {
                tools.push(tool(Land(vcs.clone())));
            }
        }
        tools
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(MarkListings(self.vcs.clone())))
    }
}

/// Marks each entry of an `ls` listing that `@` changes: a file with
/// how it changed, a directory `modified` when anything under it did.
struct MarkListings(Vcs);

#[async_trait]
impl PluginRun for MarkListings {
    async fn after_tool(
        &mut self,
        call: &ToolCall,
        output: &mut ToolOutput,
        _ctx: &PluginCtx,
    ) {
        if call.name != LS {
            return;
        }
        let Some(details) = output.details.as_mut() else {
            return;
        };
        let Some(dir) = details.get("dir").and_then(Value::as_str) else {
            return;
        };
        let Some(prefix) = repo_path(self.0.root(), Path::new(dir)) else {
            return;
        };
        // A listing without marks beats a failed call.
        let Ok(changes) = self.0.changes().await else {
            return;
        };
        mark(details, &prefix, &changes);
    }
}

/// `dir` as a path in the repository (`""` for its root, else ending in
/// `/`), when it lies inside `root`.
fn repo_path(root: &Path, dir: &Path) -> Option<String> {
    let canonical = |path: &Path| {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
    };
    let relative = canonical(dir)
        .strip_prefix(canonical(root))
        .ok()?
        .to_owned();
    let mut prefix = String::new();
    for part in relative.components() {
        prefix.push_str(part.as_os_str().to_str()?);
        prefix.push('/');
    }
    Some(prefix)
}

fn mark(details: &mut Value, prefix: &str, changes: &[FileChange]) {
    let Some(entries) =
        details.get_mut("entries").and_then(Value::as_array_mut)
    else {
        return;
    };
    for entry in entries {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let change = changes.iter().find_map(|change| {
            let rest = change.path.strip_prefix(prefix)?.strip_prefix(name)?;
            match rest {
                "" => Some(change.kind),
                rest if rest.starts_with('/') => Some(ChangeKind::Modified),
                _ => None,
            }
        });
        if let Some(kind) = change
            && let Ok(kind) = serde_json::to_value(kind)
        {
            entry["change"] = kind;
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn change(path: &str, kind: ChangeKind) -> FileChange {
        FileChange {
            path: path.to_owned(),
            kind,
        }
    }

    fn marks(prefix: &str, changes: &[FileChange]) -> Vec<Value> {
        let mut details = json!({
            "entries": [
                { "name": "src", "kind": "dir" },
                { "name": "src.rs", "kind": "file" },
                { "name": "new.rs", "kind": "file" },
                { "name": "old.rs", "kind": "file" },
            ],
        });
        mark(&mut details, prefix, changes);
        details["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry.get("change").cloned().unwrap_or(Value::Null))
            .collect()
    }

    #[test]
    fn a_file_takes_its_change_and_a_folder_modified() {
        let changes = [
            change("crates/a/src/lib.rs", ChangeKind::Added),
            change("crates/a/new.rs", ChangeKind::Added),
            change("crates/b/old.rs", ChangeKind::Modified),
        ];
        assert_eq!(
            marks("crates/a/", &changes),
            [json!("modified"), Value::Null, json!("added"), Value::Null]
        );
    }

    #[test]
    fn a_name_is_not_a_prefix_of_another() {
        // `src.rs` changing says nothing of `src/`, nor the reverse.
        let changes = [change("src.rs", ChangeKind::Modified)];
        assert_eq!(
            marks("", &changes),
            [Value::Null, json!("modified"), Value::Null, Value::Null]
        );
    }

    #[test]
    fn a_directory_outside_the_workspace_has_no_repo_path() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("crates/a");
        std::fs::create_dir_all(&inside).unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(repo_path(root.path(), root.path()).as_deref(), Some(""));
        assert_eq!(
            repo_path(root.path(), &inside).as_deref(),
            Some("crates/a/")
        );
        assert_eq!(repo_path(root.path(), outside.path()), None);
    }
}
