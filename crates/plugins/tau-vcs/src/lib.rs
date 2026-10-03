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
//!   `vcs_resolve`, `vcs_undo`.
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

#[cfg(feature = "host")]
mod clone;
#[cfg(feature = "host")]
pub mod delegate;
pub mod details;
#[cfg(feature = "host")]
mod diff;
#[cfg(feature = "host")]
pub mod error;
#[cfg(feature = "host")]
mod land;
#[cfg(feature = "host")]
mod lock;
#[cfg(feature = "host")]
mod ops;
#[cfg(feature = "host")]
pub mod plugin;
#[cfg(feature = "host")]
pub mod project;
#[cfg(feature = "host")]
pub mod run_workspace;
#[cfg(feature = "host")]
mod session;
#[cfg(feature = "host")]
pub mod tools;
pub mod ui;
#[cfg(feature = "host")]
mod vcs;

#[cfg(feature = "host")]
pub use clone::{CloneError, TransferError, clone_bare};
#[cfg(feature = "host")]
pub use delegate::Delegate;
pub use details::{ChangeInfo, ChangeKind, FileChange, Landing, TooLarge};
#[cfg(feature = "host")]
pub use diff::MAX_DIFF_BYTES;
#[cfg(feature = "host")]
pub use error::VcsError;
#[cfg(feature = "host")]
pub use ops::{
    Committed,
    DEFAULT_LOG_LIMIT,
    MAX_LOG_LIMIT,
    TurnSnapshot,
    WorkingCopy,
};
#[cfg(feature = "host")]
pub use plugin::VcsPlugin;
#[cfg(feature = "host")]
pub use project::{
    DEFAULT_WORKSPACE,
    FileDiff,
    Project,
    StackChange,
    UpdateFrom,
    Updated,
};
#[cfg(feature = "host")]
pub use run_workspace::{Link, RunWorkspace};
#[cfg(feature = "host")]
pub use session::MAX_NEW_FILE_SIZE;
#[cfg(feature = "host")]
pub use vcs::{Identity, Vcs};

/// What every tool returns when the run is cancelled before it starts.
pub const ABORTED: &str = "Operation aborted";
