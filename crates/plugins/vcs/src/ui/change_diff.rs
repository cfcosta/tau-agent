//! What a `vcs_diff` or `vcs_show` result shows: one change and its
//! diff against its parent, file by file, each file in hunks whose
//! lines carry their old and new line numbers. `vcs_show` adds the
//! author and parents.

use serde::Deserialize;
use serde_json::Value;
use tau_agent::tool::TypedTool as _;
use tau_ui_kit::diff::DiffKind;

use super::change_log::Change;
use crate::{
    ChangeInfo,
    ChangeKind,
    FileChange,
    tools::{Diff, Show},
};

/// The tool whose results read as a [`ChangeDiff`] of the files alone.
pub const DIFF_TOOL: &str = Diff::NAME;
/// The tool whose results read as a [`ChangeDiff`] with its author and
/// parents.
pub const SHOW_TOOL: &str = Show::NAME;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeDiff {
    pub change: Change,
    /// Empty for `vcs_diff`, which does not send them.
    pub parents: Vec<Change>,
    pub author: Option<Author>,
    pub files: Vec<FileDiff>,
    /// The diff was cut at 50 KB: the last file may end early and later
    /// ones are missing.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub kind: ChangeKind,
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
    /// Git said the file is binary, so it has no hunks.
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hunk {
    /// `@@ -38,7 +38,12 @@`.
    pub header: String,
    pub lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HunkLine {
    pub kind: DiffKind,
    /// The line's number before the change; none for an added line.
    pub old: Option<u32>,
    /// The line's number after the change; none for a removed line.
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Deserialize)]
struct Details {
    change: ChangeInfo,
    files: Vec<FileChange>,
    diff: String,
    truncated: bool,
    #[serde(default)]
    parents: Vec<ChangeInfo>,
    author: Option<Author>,
}

impl ChangeDiff {
    /// Reads a `vcs_diff` or `vcs_show` result's details. `None` when
    /// they are not that shape.
    pub fn parse(details: &Value) -> Option<Self> {
        let details = Details::deserialize(details).ok()?;
        let files = files_of(&details.diff, &details.files, details.truncated);
        Some(Self {
            change: Change::new(details.change),
            parents: details.parents.into_iter().map(Change::new).collect(),
            author: details.author,
            files,
            truncated: details.truncated,
        })
    }

    pub fn added(&self) -> usize {
        self.files.iter().map(|file| file.added).sum()
    }

    pub fn removed(&self) -> usize {
        self.files.iter().map(|file| file.removed).sum()
    }

    /// `+81 −18`, as the edit card sums up its diff.
    pub fn stat(&self) -> String {
        format!("+{} −{}", self.added(), self.removed())
    }

    /// `4 files`.
    pub fn file_count(&self) -> String {
        match self.files.len() {
            1 => "1 file".into(),
            count => format!("{count} files"),
        }
    }
}

/// The files of a tool's diff, each with the kind the tool's own list
/// gives it. A file with no hunks (a mode change, an empty file) comes
/// from the list; a cut diff leaves out the files past the cut.
pub fn files_of(
    diff: &str,
    changes: &[FileChange],
    truncated: bool,
) -> Vec<FileDiff> {
    let mut files = parse_files(diff);
    for change in changes {
        match files.iter_mut().find(|file| file.path == change.path) {
            Some(file) => file.kind = change.kind,
            None if !truncated => files.push(FileDiff {
                path: change.path.clone(),
                kind: change.kind,
                hunks: Vec::new(),
                added: 0,
                removed: 0,
                binary: false,
            }),
            None => {}
        }
    }
    files
}

/// Reads unified diff text, as `git diff` writes it, file by file.
pub fn parse_files(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    // The next old and new line numbers in the current hunk.
    let (mut old, mut new) = (0, 0);
    for line in diff.lines() {
        if let Some(paths) = line.strip_prefix("diff --git a/") {
            let path = paths
                .split_once(" b/")
                .map_or(paths, |(path, _)| path)
                .to_owned();
            files.push(FileDiff {
                path,
                kind: ChangeKind::Modified,
                hunks: Vec::new(),
                added: 0,
                removed: 0,
                binary: false,
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("new file mode") {
            file.kind = ChangeKind::Added;
        } else if line.starts_with("deleted file mode") {
            file.kind = ChangeKind::Removed;
        } else if line.starts_with("Binary files ") {
            file.binary = true;
        } else if line.starts_with("@@") {
            (old, new) = hunk_starts(line);
            file.hunks.push(Hunk {
                header: line.to_owned(),
                lines: Vec::new(),
            });
        } else if let Some(hunk) = file.hunks.last_mut() {
            let (kind, text) = match line.split_at_checked(1) {
                Some(("+", text)) => (DiffKind::Added, text),
                Some(("-", text)) => (DiffKind::Removed, text),
                Some((" ", text)) => (DiffKind::Context, text),
                // `\ No newline at end of file`, or an empty context
                // line an editor trimmed.
                Some(("\\", _)) => continue,
                _ => (DiffKind::Context, ""),
            };
            let (old_no, new_no) = match kind {
                DiffKind::Added => {
                    file.added += 1;
                    (None, Some(bump(&mut new)))
                }
                DiffKind::Removed => {
                    file.removed += 1;
                    (Some(bump(&mut old)), None)
                }
                DiffKind::Context => {
                    (Some(bump(&mut old)), Some(bump(&mut new)))
                }
            };
            hunk.lines.push(HunkLine {
                kind,
                old: old_no,
                new: new_no,
                text: text.to_owned(),
            });
        }
        // `---`, `+++` and mode lines before the first hunk say nothing
        // the rest does not.
    }
    files
}

fn bump(number: &mut u32) -> u32 {
    let current = *number;
    *number += 1;
    current
}

/// The first old and new line numbers of `@@ -38,7 +38,12 @@`. An
/// empty side (`-0,0`) starts at 1, where its first line would be.
fn hunk_starts(header: &str) -> (u32, u32) {
    let start = |sign: char| {
        header
            .split_whitespace()
            .find_map(|part| part.strip_prefix(sign))
            .and_then(|range| range.split(',').next()?.parse::<u32>().ok())
            .map_or(1, |start| start.max(1))
    };
    (start('-'), start('+'))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DIFF: &str = "\
diff --git a/src/retry.rs b/src/retry.rs
--- a/src/retry.rs
+++ b/src/retry.rs
@@ -38,4 +38,5 @@
     /// How long to wait.
-    pub fn delay(&self) -> Duration {
+    pub fn delay(&self, hint: Option<Duration>) -> Duration {
+        // The hint wins.
         self.base

diff --git a/tests/new.rs b/tests/new.rs
new file mode 100644
--- /dev/null
+++ b/tests/new.rs
@@ -0,0 +1,2 @@
+fn one() {}
+fn two() {}
\\ No newline at end of file
diff --git a/old.rs b/old.rs
deleted file mode 100644
--- a/old.rs
+++ /dev/null
@@ -1 +0,0 @@
-gone
diff --git a/logo.png b/logo.png
Binary files a/logo.png and b/logo.png differ
";

    #[test]
    fn reads_files_and_numbers_their_lines() {
        let files = parse_files(DIFF);
        let paths: Vec<_> = files
            .iter()
            .map(|file| {
                (file.path.as_str(), file.kind, file.added, file.removed)
            })
            .collect();
        assert_eq!(
            paths,
            [
                ("src/retry.rs", ChangeKind::Modified, 2, 1),
                ("tests/new.rs", ChangeKind::Added, 2, 0),
                ("old.rs", ChangeKind::Removed, 0, 1),
                ("logo.png", ChangeKind::Modified, 0, 0),
            ]
        );
        assert!(files[3].binary && files[3].hunks.is_empty());

        let hunk = &files[0].hunks[0];
        assert_eq!(hunk.header, "@@ -38,4 +38,5 @@");
        let numbers: Vec<_> = hunk
            .lines
            .iter()
            .map(|line| (line.kind, line.old, line.new))
            .collect();
        assert_eq!(
            numbers,
            [
                (DiffKind::Context, Some(38), Some(38)),
                (DiffKind::Removed, Some(39), None),
                (DiffKind::Added, None, Some(39)),
                (DiffKind::Added, None, Some(40)),
                (DiffKind::Context, Some(40), Some(41)),
                (DiffKind::Context, Some(41), Some(42)),
            ]
        );
        assert_eq!(hunk.lines[4].text, "        self.base");
        assert_eq!(hunk.lines[5].text, "");

        let added = &files[1].hunks[0].lines;
        assert_eq!(added.len(), 2, "the no-newline note is not a line");
        assert_eq!((added[0].old, added[0].new), (None, Some(1)));
        let removed = &files[2].hunks[0].lines;
        assert_eq!((removed[0].old, removed[0].new), (Some(1), None));
    }

    fn info(id: &str, description: &str) -> Value {
        json!({
            "change_id": id, "commit_id": format!("{id}-commit"),
            "description": description, "empty": false, "conflict": false,
            "immutable": false, "working_copy": false,
            "divergent": false, "bookmarks": [],
        })
    }

    #[test]
    fn a_show_has_its_author_and_parents() {
        let show = ChangeDiff::parse(&json!({
            "change": info("onvkmqwo", "feat(tau-ai): honor retry-after\n\nBody.\n"),
            "parents": [info("qzpxumwy", "tau: run r turn 2\n")],
            "author": { "name": "tau", "email": "tau@localhost" },
            "files": [
                { "path": "src/retry.rs", "kind": "modified" },
                { "path": "tests/new.rs", "kind": "added" },
                { "path": "old.rs", "kind": "removed" },
                { "path": "logo.png", "kind": "modified" },
            ],
            "diff": DIFF,
            "truncated": false,
        }))
        .expect("a show");
        assert_eq!(show.change.subject, "honor retry-after");
        assert_eq!(show.parents[0].scope.as_deref(), Some("tau"));
        assert_eq!(show.author.as_ref().unwrap().name, "tau");
        assert_eq!(show.stat(), "+4 −2");
        assert_eq!(show.file_count(), "4 files");
    }

    #[test]
    fn a_cut_diff_keeps_the_files_it_reached() {
        let diff = ChangeDiff::parse(&json!({
            "change": info("onvkmqwo", ""),
            "files": [
                { "path": "src/retry.rs", "kind": "modified" },
                { "path": "never.rs", "kind": "added" },
            ],
            "diff": DIFF.split("diff --git a/tests").next().unwrap(),
            "truncated": true,
        }))
        .expect("a diff");
        assert!(diff.parents.is_empty() && diff.author.is_none());
        assert_eq!(diff.files.len(), 1, "never.rs is past the cut");

        // A mode change has no hunks, yet the file still shows.
        let mode = ChangeDiff::parse(&json!({
            "change": info("onvkmqwo", ""),
            "files": [{ "path": "run.sh", "kind": "modified" }],
            "diff": "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n",
            "truncated": false,
        }))
        .unwrap();
        assert_eq!(mode.files[0].path, "run.sh");
        assert!(mode.files[0].hunks.is_empty());
    }

    #[test]
    fn other_details_are_not_a_diff() {
        assert!(
            ChangeDiff::parse(&json!({ "changes": [], "more": false }))
                .is_none()
        );
    }
}
