//! Version-control tools for tau agents, backed by jj-lib
//! (`docs/reference/vcs.md`), so the model never runs `jj` or `git` in a
//! shell.
//!
//! A [`Vcs`] opens one jj workspace ([`Vcs::open`]) or makes a new
//! repository with an internal Git store ([`Vcs::init`]).
//! [`VcsPlugin`] adds its tools to an agent:
//! `Agent::plugin(VcsPlugin::new(vcs))`.
//!
//! - Reading: `vcs_status`, `vcs_diff`, `vcs_log`, `vcs_show`.
//! - Writing: `vcs_describe`, `vcs_commit`, `vcs_new`, `vcs_restore`,
//!   `vcs_undo`.
//! - Landing, when asked for ([`VcsPlugin::landing`]): `vcs_land`.
//!
//! A [`Project`] is a repository tau owns, cloned from the user's, with
//! a workspace per run; [`RunWorkspace`] makes a run's workspace when
//! the run starts and snapshots it after each turn, so forks start from
//! a turn's code. The model makes the commits, with `vcs_commit`
//! ([ADR 0014](../../../docs/decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md)).
//!
//! The tools take change ids and commit ids, never revsets. Every tool
//! snapshots the working copy first, so edits made with other tools are
//! never lost, and the writing tools refuse immutable commits. Fetch and
//! push are left to the host; [`clone_bare`] brings a remote repository
//! in, over HTTPS, for [`Project::import`].

mod clone;
pub mod delegate;
mod diff;
pub mod error;
mod land;
mod lock;
mod ops;
pub mod plugin;
pub mod project;
pub mod run_workspace;
mod session;
pub mod tools;
pub mod ui;
mod vcs;

pub use clone::{CloneError, TransferError, clone_bare};
pub use delegate::Delegate;
pub use diff::{ChangeKind, FileChange, MAX_DIFF_BYTES};
pub use error::VcsError;
pub use land::Landing;
pub use ops::{
    ChangeInfo,
    Committed,
    DEFAULT_LOG_LIMIT,
    MAX_LOG_LIMIT,
    TurnSnapshot,
    WorkingCopy,
};
pub use plugin::VcsPlugin;
pub use project::{
    DEFAULT_WORKSPACE,
    FileDiff,
    Project,
    StackChange,
    UpdateFrom,
    Updated,
};
pub use run_workspace::{Link, RunWorkspace};
pub use session::{MAX_NEW_FILE_SIZE, TooLarge};
pub use vcs::{Identity, Vcs};

/// What every tool returns when the run is cancelled before it starts.
pub const ABORTED: &str = "Operation aborted";
