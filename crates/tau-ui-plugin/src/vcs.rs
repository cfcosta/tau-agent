//! What the kernel knows of version control (ADR 0045): a run lands on
//! its parent (ADR 0009), and the VCS slot's plugin (tau-vcs, on jj)
//! says how. These are the shapes a landing has, and the tools a run
//! proposes its landing and starts a sub-agent with.

use serde::{Deserialize, Serialize};
use tau_agent::tool::RunId;

use crate::CallData;

/// The tool a run proposes its landing with (ADR 0014).
pub const LAND: &str = "vcs_land";
/// The tool the model starts a sub-agent with.
pub const SPAWN: &str = "spawn";

/// The sub-agent a `spawn` call started.
pub fn spawned(data: &CallData) -> Option<RunId> {
    let result = data.result.as_ref().filter(|result| !result.error)?;
    let run = result.details.as_ref()?.get("run")?.as_str()?;
    Some(RunId(run.into()))
}

/// One change, as the tools describe it in `details`.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
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

/// What landing a child did, or would do.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
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

/// The record a landing leaves in its parent:
/// enough to draw its card again when the parent comes back from
/// history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandingRecord {
    pub from: String,
    pub title: String,
    pub landing: Landing,
    /// tau closed in the middle of the landing, and finished it at its
    /// next start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recovered: bool,
    /// The chat landed what it had and went on (ADR 0034): it did not
    /// close.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kept: bool,
}
