//! What the kernel knows of version control (ADR 0045): a run lands on
//! its parent (ADR 0009), and the VCS slot's plugin (tau-vcs, on jj)
//! says how. These are the shapes a landing has, the tools a run
//! proposes its landing and starts a sub-agent with, and the [`VCS`]
//! slot the host reaches a repository's code through.

use std::{
    any::Any,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use tau_agent::{event::LimitKind, tool::RunId};

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

/// One file's change between two commits ([`Repository::diff`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileDiff {
    pub path: String,
    pub kind: ChangeKind,
    pub added: usize,
    pub removed: usize,
    /// The file's unified diff, as `git diff` writes it.
    pub text: String,
}

/// One change on a run's stack, or on trunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackChange {
    pub commit_id: String,
    pub change_id: String,
    pub description: String,
    pub conflict: bool,
}

/// Where to push: the remote's URL, and the token that signs in to it
/// over HTTPS (GitHub reads it as the password of any user).
#[derive(Debug, Clone)]
pub struct Remote {
    pub url: String,
    pub token: Option<String>,
}

/// What pushing trunk pushed: trunk's branch, the remote's commit
/// before and after (full hex ids), and the changes it took, oldest
/// first. Nothing was pushed when `changes` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub branch: String,
    pub from: Option<String>,
    pub to: String,
    pub changes: Vec<StackChange>,
}

/// Why pushing trunk failed ([`Repository::push_trunk`]).
#[derive(Debug)]
pub enum PushError {
    /// The remote's branch moved since the last fetch: fetching puts
    /// trunk's `ahead` commits on top of the remote's, and the push can
    /// go again.
    Moved {
        branch: String,
        ahead: u32,
    },
    Failed(anyhow::Error),
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Moved { branch, .. } => {
                write!(f, "the remote's {branch} moved since the last fetch")
            }
            Self::Failed(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for PushError {}

/// Where a run's later commits replay ([`Repository::replay_run`]): the
/// remote branch they went to before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct After {
    /// The branch's commit on the remote, in hex.
    pub head: String,
    /// The change id of the run's last commit in it.
    pub change_id: String,
}

/// A run's commits replayed for a pull request
/// ([`Repository::replay_run`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replayed {
    /// The commit they went onto, in hex.
    pub onto: String,
    /// The commits that replayed, oldest first.
    pub commits: Vec<ReplayedCommit>,
    /// How many files the copies change, all together.
    pub changed: usize,
}

/// One of a run's commits, and its copy on the remote's trunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayedCommit {
    pub change: StackChange,
    /// The copy, in hex.
    pub copy: String,
    /// The lines the copy adds and removes.
    pub added: u32,
    pub removed: u32,
}

/// The trunk before and after an update, as full commit ids in hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updated {
    pub before: String,
    pub after: String,
}

impl Updated {
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
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
    /// A repository the person works on in place, set up before (ADR
    /// 0043): its folder, the main chat's workspace and where runs'
    /// workspaces go.
    Local {
        folder: PathBuf,
        main_dir: PathBuf,
        runs_dir: PathBuf,
    },
}

/// A folder on this computer that can be added as a repository worked
/// in place ([`Vcs::inspect`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFolder {
    /// The repository's folder, with symbolic links resolved.
    pub root: PathBuf,
    /// The URL of its `origin` remote, when it has one.
    pub origin: Option<String>,
}

/// A step of setting a repository up in place ([`Vcs::setup_local`]),
/// heard before it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupStep {
    /// Which step it is, from 0.
    pub at: usize,
    /// How many steps a setup has.
    pub of: usize,
    /// What it does, such as "snapshotting the working copy".
    pub doing: String,
}

/// What keeps the repositories' code: what fills [`VCS`].
pub trait Vcs: Send + Sync {
    /// The project at `root`, opened, or made as `how` says.
    fn open<'a>(
        &'a self,
        root: &'a Path,
        how: Open,
    ) -> HostFuture<'a, anyhow::Result<Arc<dyn Repository>>>;

    /// Whether `root` holds a project.
    fn is_project(&self, root: &Path) -> bool;

    /// Makes a project at `root`, which holds none, by cloning `from`.
    /// On failure `root` is removed.
    fn clone_repository<'a>(
        &'a self,
        from: Remote,
        root: &'a Path,
    ) -> HostFuture<'a, anyhow::Result<()>>;

    /// What the folder `folder` is, as a repository to add in place, or
    /// why it cannot be added: the plugin's refusal, which says what to
    /// add instead.
    fn inspect<'a>(
        &'a self,
        folder: &'a Path,
    ) -> HostFuture<'a, anyhow::Result<LocalFolder>>;

    /// A name for the runs' directory of the repository at `root`, under
    /// `runs`, which no directory there has.
    fn runs_dir_name(&self, runs: &Path, root: &Path) -> String;

    /// Sets the repository at `folder` up to work in where it is (ADR
    /// 0043), all or nothing: the main chat's workspace at `main_dir`
    /// and runs' under `runs_dir`. `progress` hears of each step before
    /// it starts, and stops the setup by breaking.
    fn setup_local<'a>(
        &'a self,
        folder: &'a Path,
        main_dir: PathBuf,
        runs_dir: PathBuf,
        progress: Box<dyn FnMut(SetupStep) -> ControlFlow<()> + Send>,
    ) -> HostFuture<'a, anyhow::Result<Arc<dyn Repository>>>;

    /// What a caller reads about its sub-agent's landing, after the
    /// sub-agent's answer: whether `limit` cut it short, how many changes
    /// landed, and the paths it left in conflict (`brought`).
    fn landing_note(
        &self,
        landing: &Landing,
        brought: &[String],
        limit: Option<LimitKind>,
    ) -> String;

    /// What a caller reads about a sub-agent kept off its work, after its
    /// answer: whether `limit` cut it short, and its `changes`.
    fn kept_note(&self, changes: &[String], limit: Option<LimitKind>)
    -> String;
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

    /// Where the project keeps its repository and runs' workspaces.
    fn root(&self) -> &Path;

    /// The name of trunk's branch, such as `main`.
    fn trunk_name(&self) -> HostFuture<'_, anyhow::Result<String>>;

    /// Trunk's commits the remote lacks, as the last fetch or push left
    /// it, oldest first: the main chat's work waiting to be pushed.
    fn unpushed(&self) -> HostFuture<'_, anyhow::Result<Vec<StackChange>>>;

    /// The commit `run`'s head names, in hex: none when it has no head,
    /// or its work was dropped.
    fn run_head<'a>(
        &'a self,
        run: &'a RunId,
    ) -> HostFuture<'a, anyhow::Result<Option<String>>>;

    /// `links` with each commit moved to where its change is now, as a
    /// rewrite leaves it. A turn's snapshot stays the commit it is.
    fn current(
        &self,
        links: Vec<Link>,
    ) -> HostFuture<'_, anyhow::Result<Vec<Link>>>;

    /// How the files differ from commit `from` to commit `to` (full hex
    /// ids): one entry per changed file, in path order.
    fn diff<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Vec<FileDiff>>>;

    /// Brings in what the remote `from` has: new commits, and trunk where
    /// the remote has it now. Runs keep their workspaces and commits.
    fn update(&self, from: Remote) -> HostFuture<'_, anyhow::Result<Updated>>;

    /// Pushes trunk to the remote's branch of the same name, as a
    /// fast-forward: [`PushError::Moved`] when the remote's branch moved
    /// since the last fetch.
    fn push_trunk(
        &self,
        to: Remote,
    ) -> HostFuture<'_, Result<Pushed, PushError>>;

    /// `run`'s commits replayed onto the remote's trunk as the last fetch
    /// left it, so a pull request carries only the run's own work. With
    /// `after`, only the commits after the one it names replay, onto its
    /// branch. Refuses a run with no commits, and a replay that would
    /// conflict, naming its files.
    fn replay_run<'a>(
        &'a self,
        run: &'a RunId,
        after: Option<After>,
    ) -> HostFuture<'a, anyhow::Result<Replayed>>;

    /// Points the remote's branch `branch` at `head` (a full commit id in
    /// hex) if the remote has it at `expected` (absent when `None`).
    /// Returns the commit pushed, which signing on push may have copied.
    fn push_branch(
        &self,
        to: Remote,
        branch: String,
        expected: Option<String>,
        head: String,
    ) -> HostFuture<'_, anyhow::Result<String>>;

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

    /// Itself, for the run assembly that still reaches the jj project
    /// behind it (ADR 0045 step 4), and tests; it goes, and this with it.
    fn as_any(&self) -> &dyn Any;
}
