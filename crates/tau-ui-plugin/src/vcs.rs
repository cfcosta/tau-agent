//! What the kernel knows of version control (ADR 0045): a run lands on
//! its parent (ADR 0009), and the VCS slot's plugin (tau-vcs, on jj)
//! says how. These are the shapes a landing has, the tools a run
//! proposes its landing and starts a sub-agent with, and the [`VCS`]
//! slot the host reaches a repository's code through.

use std::{
    any::Any,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use tau_agent::tool::RunId;

use crate::{CallData, registry::HostFuture, slots::Slot};

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

/// The plugin name a run's links are stored under ([`Link`]).
pub const WORKSPACE_PLUGIN: &str = "tau-vcs";

/// A turn, and the commit that holds the files it left. Stored as the
/// plugin's record after each turn, and for each change a landing
/// brought.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub turn: u32,
    pub workspace: String,
    pub commit_id: String,
    pub change_id: String,
    /// The turn changed files.
    pub changed: bool,
    /// The child run whose landing put this change on the run's stack
    /// (ADR 0009); none for the run's own turns. A landing stores one
    /// link per change it brought, all with the run's latest turn.
    pub from: Option<String>,
    /// The commit is a snapshot of `@` at the end of the turn, found by
    /// its commit id: `@` moves on under the same change id (ADR 0014).
    /// Otherwise it is a change, found by its change id.
    #[serde(default)]
    pub snapshot: bool,
}

impl Link {
    /// Reads a link back from a stored record body.
    pub fn parse(body: &str) -> Option<Self> {
        serde_json::from_str(body).ok()
    }
}

/// The slot of the plugin that keeps the repositories' code.
pub const VCS: Slot<dyn Vcs> = Slot::new("tau.vcs");

/// How [`Vcs::open`] comes to a repository's project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Open {
    /// The project is there already: a clone tau made.
    Existing,
    /// Made empty the first time, with these files, by path and text:
    /// tau's own repositories.
    OrInit(Vec<(String, String)>),
    /// Made the first time from the repository at this path.
    OrImport(PathBuf),
}

/// What keeps the repositories' code: what fills [`VCS`].
pub trait Vcs: Send + Sync {
    /// The project at `root`, opened, or made as `how` says.
    fn open<'a>(
        &'a self,
        root: &'a Path,
        how: Open,
    ) -> HostFuture<'a, anyhow::Result<Arc<dyn Repository>>>;
}

/// What a run's commits land on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Onto {
    /// Trunk's branch: the main chat's, which commits on it.
    Trunk,
    /// The run's own head.
    Run(RunId),
}

/// A run's newest commit, as a landing reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Newest {
    /// Its id, in hex.
    Commit(String),
    /// Its workspace holds work not committed, or files too large to
    /// keep: it has none to land yet.
    Uncommitted,
}

/// A landing to read or carry out ([`Repository::land`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Land {
    /// The child's newest commit, in hex.
    pub child: String,
    /// The parent's workspace, by name, where it lands.
    pub workspace: String,
    /// The parent's head, which moves to the landed commits.
    pub onto: Onto,
    /// It lands; otherwise it changes nothing and says what landing
    /// would do.
    pub confirm: bool,
}

/// A run thrown away ([`Repository::discard`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discard {
    pub run: RunId,
    /// Its parent's head: what the parent has stays.
    pub onto: Onto,
    /// Its workspace, by name, if it has one: it goes.
    pub workspace: Option<String>,
}

/// Where a run stands, as far as its workspaces and head go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// It keeps them: running, or open, as a chat that can go on, or a
    /// sub-agent whose workspace was kept for recovery.
    Kept,
    /// Its work is on its parent now: they go, its commits stay.
    Landed,
    /// Its work is thrown away, as for a dropped chat or a sub-agent
    /// that failed or was cut off: they go, and so do its own commits.
    Discarded,
}

/// What a discarded run's commits are measured against: what its
/// parent has stays, and so does trunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keep {
    /// What this run's head has: the parent run's.
    Run(RunId),
    /// What this workspace's working copy has: the main chat's, which
    /// commits on trunk, and whose own commits trunk may be past.
    Workspace(String),
}

/// One run that may own workspaces and a head in a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// The run's id.
    pub run: String,
    pub standing: Standing,
    /// The workspaces it worked in, by name.
    pub workspaces: Vec<String>,
    /// For a discarded run: what stays when its commits go.
    pub keep: Keep,
}

/// What a repository holds of runs: the workspaces they work in, never
/// the main chat's nor one the person made, and the names of their
/// heads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    pub workspaces: Vec<String>,
    pub heads: Vec<String>,
}

/// One repository's project, as the kernel asks it: its workspaces, and
/// its runs' landings.
pub trait Repository: Send + Sync {
    /// The name of the main chat's workspace.
    fn main_workspace(&self) -> &str;

    /// Where the workspace `name` is, whether or not it exists.
    fn workspace_dir(&self, name: &str) -> PathBuf;

    /// The names of the workspaces runs have ([`Held::workspaces`]).
    fn workspaces(&self) -> HostFuture<'_, anyhow::Result<Vec<String>>>;

    /// Brings the main chat's workspace `workspace` up to trunk, which
    /// moves without it on an update: its work goes onto trunk's head,
    /// so its next commit moves trunk forward, not aside. A workspace
    /// that is not there is left be.
    fn catch_up<'a>(
        &'a self,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<()>>;

    /// The paths `workspace`'s working copy holds in conflict.
    fn conflicts<'a>(
        &'a self,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Vec<String>>>;

    /// `run`'s newest commit, which works in `workspace`: its head's, or
    /// the one its working copy stands on when it has no head.
    fn newest<'a>(
        &'a self,
        run: &'a RunId,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Newest>>;

    /// Makes the workspace `name` on trunk, unless it is there.
    fn ready_workspace<'a>(
        &'a self,
        name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<()>>;

    /// Lands a child's commits on its parent (ADR 0009), or says what
    /// landing them would do.
    fn land(&self, land: Land) -> HostFuture<'_, anyhow::Result<Landing>>;

    /// What the confirmed landing of the child whose newest commit was
    /// `child` did, if it happened: for finishing a landing tau closed
    /// in the middle of.
    fn landed<'a>(
        &'a self,
        child: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Option<Landing>>>;

    /// Removes these run workspaces, by name. Their commits stay.
    fn forget_workspaces(
        &self,
        names: Vec<String>,
    ) -> HostFuture<'_, anyhow::Result<()>>;

    /// Removes `run`'s head. Its commits stay.
    fn forget_run<'a>(
        &'a self,
        run: &'a RunId,
    ) -> HostFuture<'a, anyhow::Result<()>>;

    /// Throws a run's work away: its own commits, those its parent lacks,
    /// its workspace and its head.
    fn discard(&self, discard: Discard) -> HostFuture<'_, anyhow::Result<()>>;

    /// What the repository holds of runs now.
    fn held(&self) -> HostFuture<'_, anyhow::Result<Held>>;

    /// Removes from `held` what no kept run of `owners` has, and the own
    /// commits of discarded runs (`docs/reference/vcs.md`, "Sweeping at
    /// start").
    fn sweep(
        &self,
        owners: Vec<Owner>,
        held: Held,
    ) -> HostFuture<'_, anyhow::Result<()>>;

    /// Itself, for the callers that still reach the jj project behind
    /// it (ADR 0045 steps 4 and 6b); they go, and this with them.
    fn as_any(&self) -> &dyn Any;
}
