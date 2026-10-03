//! What the tools put in their results' details, and what landing a
//! run does: the shapes the cards read, with or without the host half.

use serde::{Deserialize, Serialize};

/// The names the model calls the reading tools by.
pub const STATUS: &str = "vcs_status";
pub const DIFF: &str = "vcs_diff";
pub const LOG: &str = "vcs_log";
pub const SHOW: &str = "vcs_show";
/// The name the model calls the sub-agent tool by.
pub const DELEGATE: &str = "delegate";

/// One change, as the tools describe it in `details`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeInfo {
    /// The full change id, in jj's `k`-`z` letters.
    pub change_id: String,
    /// The full commit id, in hex.
    pub commit_id: String,
    pub description: String,
    /// The change touches no files.
    pub empty: bool,
    pub conflict: bool,
    pub immutable: bool,
    /// The change is this workspace's working copy (`@`).
    pub working_copy: bool,
    /// More than one visible commit has this change id, so the change id
    /// names none of them: pass a commit id.
    pub divergent: bool,
    /// The local bookmarks on this commit, such as `main`, sorted.
    pub bookmarks: Vec<String>,
}

/// How a path changed between two trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

impl ChangeKind {
    /// The one-letter code `jj status` uses.
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Removed => 'D',
        }
    }
}

/// One changed path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
}

/// A new file the snapshot left out of `@` for its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TooLarge {
    pub path: String,
    /// Its size in bytes.
    pub size: u64,
}

/// What landing a child did, or would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Landing {
    /// The child's changes as they are on the parent's stack after the
    /// landing, newest first. Empty when the child had nothing the
    /// parent lacks.
    pub changes: Vec<ChangeInfo>,
    /// Unresolved paths in the parent's new newest commit, whether jj's
    /// native materialization contains markers or not.
    pub conflicts: Vec<String>,
    /// The parent's newest commit after the landing, in hex.
    pub head: String,
}
