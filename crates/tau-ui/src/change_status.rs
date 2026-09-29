//! What a `vcs_status` result shows: what the working-copy change
//! (`@`) holds. Its parents, the files it changes with their diffs,
//! the files still holding conflict markers, and the new files too
//! large to snapshot.

use serde::Deserialize;
use serde_json::Value;
use tau_agent::tool::TypedTool as _;
use tau_vcs::{ChangeInfo, ChangeKind, FileChange, TooLarge, tools::Status};

use crate::{
    change_diff::{FileDiff, files_of},
    change_log::Change,
};

/// The tool whose results read as a [`ChangeStatus`].
pub const TOOL: &str = Status::NAME;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeStatus {
    pub working_copy: Change,
    /// One for a change, two or more for a merge.
    pub parents: Vec<Change>,
    /// What `@` changes against its parents, each with its hunks.
    pub files: Vec<FileDiff>,
    /// Paths that still hold conflict markers.
    pub conflicts: Vec<String>,
    /// New files over 1 MiB, left out of `@`.
    pub too_large: Vec<TooLarge>,
    /// The diff was cut at 50 KB.
    pub truncated: bool,
}

#[derive(Deserialize)]
struct Details {
    working_copy: ChangeInfo,
    parents: Vec<ChangeInfo>,
    changes: Vec<FileChange>,
    conflicts: Vec<String>,
    too_large: Vec<TooLarge>,
    diff: String,
    truncated: bool,
}

impl ChangeStatus {
    /// Reads a `vcs_status` result's details. `None` when they are not
    /// that shape.
    pub fn parse(details: &Value) -> Option<Self> {
        let details = Details::deserialize(details).ok()?;
        Some(Self {
            working_copy: Change::new(details.working_copy),
            parents: details.parents.into_iter().map(Change::new).collect(),
            files: files_of(&details.diff, &details.changes, details.truncated),
            conflicts: details.conflicts,
            too_large: details.too_large,
            truncated: details.truncated,
        })
    }

    /// `4 files`, or `empty` when `@` changes nothing.
    pub fn summary(&self) -> String {
        match self.files.len() {
            0 => "empty".into(),
            1 => "1 file".into(),
            count => format!("{count} files"),
        }
    }

    /// How many files `@` adds, modifies and deletes, leaving out the
    /// kinds it has none of.
    pub fn counts(&self) -> Vec<(ChangeKind, usize)> {
        [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Removed]
            .into_iter()
            .map(|kind| {
                (
                    kind,
                    self.files.iter().filter(|file| file.kind == kind).count(),
                )
            })
            .filter(|(_, count)| *count > 0)
            .collect()
    }

    pub fn is_conflicted(&self, path: &str) -> bool {
        self.conflicts.iter().any(|conflict| conflict == path)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn info(id: &str, description: &str, working_copy: bool) -> Value {
        json!({
            "change_id": id, "commit_id": format!("{id}-commit"),
            "description": description, "empty": false, "conflict": false,
            "immutable": !working_copy, "working_copy": working_copy,
            "divergent": false,
            "bookmarks": if working_copy { vec![] } else { vec!["main"] },
        })
    }

    const DIFF: &str = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,2 @@
 keep
-old
+new
diff --git a/src/b.rs b/src/b.rs
new file mode 100644
--- /dev/null
+++ b/src/b.rs
@@ -0,0 +1 @@
+born
";

    #[test]
    fn reads_what_the_working_copy_holds() {
        let status = ChangeStatus::parse(&json!({
            "working_copy": info("wwwwwwww", "", true),
            "parents": [info("pppppppp", "fix(tau-ui): a bubble\n", false)],
            "changes": [
                { "path": "src/a.rs", "kind": "modified" },
                { "path": "src/b.rs", "kind": "added" },
                { "path": "run.sh", "kind": "modified" },
            ],
            "conflicts": ["src/a.rs"],
            "too_large": [{ "path": "big.har", "size": 3_565_158 }],
            "diff": DIFF,
            "truncated": false,
        }))
        .expect("a status");
        assert_eq!(status.parents[0].info.bookmarks, ["main"]);
        let files: Vec<_> = status
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.hunks.len()))
            .collect();
        // run.sh changed only its mode: no hunks, still listed.
        assert_eq!(files, [("src/a.rs", 1), ("src/b.rs", 1), ("run.sh", 0)]);
        assert_eq!(status.summary(), "3 files");
        assert_eq!(
            status.counts(),
            [(ChangeKind::Added, 1), (ChangeKind::Modified, 2)]
        );
        assert!(status.is_conflicted("src/a.rs"));
        assert!(!status.is_conflicted("src/b.rs"));
        assert_eq!(status.too_large[0].size, 3_565_158);
    }

    #[test]
    fn an_empty_working_copy_says_so() {
        let status = ChangeStatus::parse(&json!({
            "working_copy": info("wwwwwwww", "", true),
            "parents": [info("pppppppp", "", false)],
            "changes": [], "conflicts": [], "too_large": [],
            "diff": "", "truncated": false,
        }))
        .unwrap();
        assert!(status.files.is_empty());
        assert_eq!(status.summary(), "empty");
        assert!(status.counts().is_empty());
    }

    #[test]
    fn other_details_are_not_a_status() {
        assert!(
            ChangeStatus::parse(&json!({ "changes": [], "more": false }))
                .is_none()
        );
    }
}
