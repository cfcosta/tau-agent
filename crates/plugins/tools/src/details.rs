//! What `ls` puts in its result's details: the listing its card draws,
//! with or without the host half.

use serde::{Deserialize, Serialize};

/// What `ls` found, for callers that draw it (`docs/reference/tools.md`,
/// "ls"). The model never sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// The directory listed, resolved against the root.
    pub dir: String,
    /// The entries the model got, in its order.
    pub entries: Vec<Entry>,
    /// The listing stopped at the entry limit or the byte cap.
    pub truncated: bool,
}

/// One entry of a [`Listing`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// A file's size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// When it last changed, in seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
    /// How many entries a directory holds, when it can be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<u64>,
    /// Where a symlink points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `.gitignore` leaves it out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignored: bool,
}

/// What an entry is. A symlink says whether it leads to a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    SymlinkDir,
}

impl EntryKind {
    pub fn is_dir(self) -> bool {
        matches!(self, Self::Dir | Self::SymlinkDir)
    }
}
