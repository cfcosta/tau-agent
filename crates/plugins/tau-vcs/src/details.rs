//! What the tools put in their results' details, and what landing a
//! run does: the shapes the cards read, with or without the host half.

use serde::{Deserialize, Serialize};
pub use tau_ui_plugin::vcs::{ChangeInfo, Landing, SPAWN};

/// The names the model calls the reading tools by.
pub const STATUS: &str = "vcs_status";
pub const DIFF: &str = "vcs_diff";
pub const LOG: &str = "vcs_log";
pub const SHOW: &str = "vcs_show";
/// The name the model commits with.
pub const COMMIT: &str = "vcs_commit";

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
    /// The largest new file the snapshot takes, in bytes: the jj
    /// config's `snapshot.max-new-file-size`.
    pub max_size: u64,
}

/// `bytes` as jj writes a size: one decimal and a binary prefix, as in
/// `1.0MiB` (jj-lib's `HumanByteSize`).
pub fn byte_size(bytes: u64) -> String {
    const PREFIXES: [&str; 9] =
        ["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "Zi", "Yi"];
    let mut at = 0;
    let mut scaled = bytes as f32;
    while scaled.abs() >= 1000.0 && at < PREFIXES.len() - 1 {
        at += 1;
        scaled /= 1024.0;
    }
    format!("{scaled:.1}{}B", PREFIXES[at])
}
