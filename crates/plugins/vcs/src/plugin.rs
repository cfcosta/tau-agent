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
    plugin::{
        Plugin,
        PluginCtx,
        PluginError,
        PluginRun,
        RunPlan,
        ToolResultView,
    },
    tool::{AgentTool, ToolOutput},
};

use crate::{
    ChangeKind,
    FileChange,
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
        crate::ui::NAME
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
    async fn after_tool_result(
        &mut self,
        view: &ToolResultView<'_>,
        output: &mut ToolOutput,
        _ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        let call = view.call;
        if call.name != LS {
            return Ok(());
        }
        let Some(details) = output.details.as_mut() else {
            return Ok(());
        };
        let Some(dir) = details.get("dir").and_then(Value::as_str) else {
            return Ok(());
        };
        let Some(prefix) = repo_path(self.0.root(), Path::new(dir)) else {
            return Ok(());
        };
        // A listing without marks beats a failed call.
        let Ok(changes) = self.0.changes().await else {
            return Ok(());
        };
        mark(details, &prefix, &changes);
        Ok(())
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
        let path = format!("{prefix}{name}");
        let exact = changes
            .iter()
            .find(|change| change.path == path)
            .map(|change| change.kind);
        let below = changes.iter().any(|change| {
            change
                .path
                .strip_prefix(&path)
                .is_some_and(|rest| rest.starts_with('/'))
        });
        let dir = entry.get("kind").and_then(Value::as_str) == Some("dir");
        let change = match exact {
            // A directory that was a file has the file's removal at its
            // own path: it is `modified` all the same.
            Some(_) if dir => Some(ChangeKind::Modified),
            Some(kind) => Some(kind),
            None => below.then_some(ChangeKind::Modified),
        };
        if let Some(kind) = change
            && let Ok(kind) = serde_json::to_value(kind)
        {
            entry["change"] = kind;
        }
    }
}

#[cfg(test)]
mod tests {
    use hegel::generators as gs;
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

    /// A tree: each file's path and contents.
    type Tree = std::collections::BTreeMap<String, u8>;

    /// A tree of a few files over a small alphabet, so that names share
    /// prefixes (`a`, `a.rs`) and a path is a file in one tree and a
    /// directory in another. No file lies under another.
    #[hegel::composite]
    fn tree(tc: &hegel::TestCase) -> Tree {
        let segment = || gs::sampled_from(vec!["a", "b", "a.rs"]);
        let paths: Vec<Vec<&str>> = tc.draw(
            gs::vecs(gs::vecs(segment()).min_size(1).max_size(3)).max_size(6),
        );
        let mut tree = Tree::new();
        for path in paths {
            let path = path.join("/");
            let blocked = tree.keys().any(|file| {
                under(&path, file) || under(file, &path) || *file == path
            });
            if !blocked {
                let contents = tc.draw(gs::integers::<u8>().max_value(2));
                tree.insert(path, contents);
            }
        }
        tree
    }

    /// `path` lies strictly under the directory `dir`.
    fn under(path: &str, dir: &str) -> bool {
        path.strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
    }

    /// The changes from `before` to `after`, in jj's path order: by
    /// component, so `a/b` sorts before `a.rs`.
    fn diff(before: &Tree, after: &Tree) -> Vec<FileChange> {
        let mut paths: Vec<&String> =
            before.keys().chain(after.keys()).collect();
        paths.sort_by(|x, y| x.split('/').cmp(y.split('/')));
        paths.dedup();
        paths
            .into_iter()
            .filter_map(|path| {
                let kind = match (before.get(path), after.get(path)) {
                    (None, Some(_)) => ChangeKind::Added,
                    (Some(_), None) => ChangeKind::Removed,
                    (Some(old), Some(new)) if old != new => {
                        ChangeKind::Modified
                    }
                    _ => return None,
                };
                Some(change(path, kind))
            })
            .collect()
    }

    /// Each entry of an `ls` of `prefix` in the tree `after` takes the
    /// mark the reference gives it (`docs/reference/vcs.md`, "ls"),
    /// read off the trees and not the change list: a file is `added`
    /// or `modified` as it differs from `before`, a directory
    /// `modified` when anything at or under it does.
    #[hegel::test(test_cases = 500)]
    fn a_listing_is_marked_as_the_reference_says(tc: hegel::TestCase) {
        let before = tc.draw(tree());
        let after = tc.draw(tree());
        let prefix: &str = tc.draw(gs::sampled_from(vec!["", "a/", "b/a/"]));

        let mut names: Vec<(String, bool)> = Vec::new();
        for path in after.keys() {
            let Some(rest) = path.strip_prefix(prefix) else {
                continue;
            };
            let (name, dir) = match rest.split_once('/') {
                Some((name, _)) => (name, true),
                None => (rest, false),
            };
            if !names.iter().any(|(seen, _)| seen == name) {
                names.push((name.to_owned(), dir));
            }
        }
        let entries: Vec<Value> = names
            .iter()
            .map(|(name, dir)| {
                json!({ "name": name, "kind": if *dir { "dir" } else { "file" } })
            })
            .collect();
        let mut details = json!({ "entries": entries });
        mark(&mut details, prefix, &diff(&before, &after));

        for ((name, dir), entry) in
            names.iter().zip(details["entries"].as_array().unwrap())
        {
            let path = format!("{prefix}{name}");
            let expected = if *dir {
                let differs = |x: &Tree, y: &Tree| {
                    x.iter().any(|(file, contents)| {
                        (*file == path || under(file, &path))
                            && y.get(file) != Some(contents)
                    })
                };
                (differs(&before, &after) || differs(&after, &before))
                    .then_some(json!("modified"))
            } else {
                match before.get(&path) {
                    None => Some(json!("added")),
                    Some(old) if *old != after[&path] => {
                        Some(json!("modified"))
                    }
                    Some(_) => None,
                }
            };
            assert_eq!(entry.get("change"), expected.as_ref(), "{path}");
        }
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
