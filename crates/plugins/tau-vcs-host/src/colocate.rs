//! A project's Git store is colocated with its main workspace, as
//! `jj git init --colocate` makes one: `main/.git` beside `main/.jj`,
//! and each run's workspace is a worktree of it ([`crate::worktree`]).
//! The jj CLI keeps such a store in step after each command; jj-lib
//! leaves that to its caller, so every operation tau writes goes
//! through [`commit`] or [`commit_in`], which do the same: bookmarks
//! and tags go out as Git's branches and tags, and Git's `HEAD` and
//! index follow the working copy of the workspace the operation was
//! written in, so `git` there sees what jj does. They also tag the
//! operation as tau's ([`crate::operations`]).

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use jj_lib::{
    commit::Commit,
    git::{export_refs, get_git_backend, reset_head},
    ref_name::WorkspaceName,
    repo::{ReadonlyRepo, Repo as _},
    transaction::Transaction,
};
use pollster::block_on;

use crate::{error::VcsError, lock::GitLock};

/// Keeps `.jj/` out of Git's view in the colocated `workspace_root`, as
/// the jj CLI does: a `.jj/.gitignore` that ignores all it holds.
/// jj-lib writes none, and without it `git status` lists `.jj/` as
/// untracked.
pub(crate) fn ignore_jj_dir(workspace_root: &Path) -> Result<(), VcsError> {
    let path = workspace_root.join(".jj").join(".gitignore");
    std::fs::write(&path, "/*\n")
        .map_err(|source| VcsError::Create { path, source })
}

/// Commits `tx` as tau's operation described `tau: <what>`, with the
/// bookmarks and tags exported to the Git store first. No workspace's
/// `HEAD` moves: an operation that changes what a workspace has checked
/// out goes through [`commit_in`]. `_git` is jj-cli's Git import/export
/// lock, held from before any import the transaction made until the
/// operation is written, as the CLI holds it.
pub(crate) fn commit(
    mut tx: Transaction,
    what: impl AsRef<str>,
    _git: &GitLock,
) -> Result<Arc<ReadonlyRepo>, VcsError> {
    let description = crate::operations::tag(&mut tx, what.as_ref());
    export(&mut tx)?;
    Ok(block_on(tx.commit(description))?)
}

/// [`commit`], for an operation written in workspace `name`, whose
/// root is `root`: when its working copy moved, Git's `HEAD` there
/// points at the new `@-` and the index is reset to it, as the jj CLI
/// keeps the workspace it runs in. Only that workspace's: one whose
/// `@` the operation rewrote from elsewhere keeps its `HEAD` until its
/// own files move to the new commit ([`reset`]).
pub(crate) fn commit_in(
    mut tx: Transaction,
    what: impl AsRef<str>,
    git: &GitLock,
    name: &WorkspaceName,
    root: &Path,
) -> Result<Arc<ReadonlyRepo>, VcsError> {
    if let Some(wc) = tx.repo().view().get_wc_commit_id(name).cloned()
        && tx.base_repo().view().get_wc_commit_id(name) != Some(&wc)
    {
        let wc = tx.repo().store().get_commit(&wc)?;
        keep_in_step(&mut tx, name, root, &wc)?;
    }
    commit(tx, what, git)
}

/// Points Git's `HEAD` in workspace `name`, at `root`, at `wc`'s parent
/// and resets the index to it, after the workspace's files moved to
/// `wc` with no operation of their own: a stale working copy brought up
/// to date, or a worktree made again ([`crate::worktree`]). Writes an
/// operation, `tau: reset git head` as jj-cli's is `reset git head`,
/// only when the view's record of that `HEAD` changed; it is a
/// snapshot's kind, which `vcs_undo` passes over. Returns the repo at
/// the newest operation.
pub(crate) fn reset(
    repo: &Arc<ReadonlyRepo>,
    name: &WorkspaceName,
    root: &Path,
    wc: &Commit,
    git: &GitLock,
) -> Result<Arc<ReadonlyRepo>, VcsError> {
    let mut tx = repo.start_transaction();
    tx.set_is_snapshot(true);
    tx.set_workspace_name(name);
    keep_in_step(&mut tx, name, root, wc)?;
    if !tx.repo().has_changes() {
        return Ok(repo.clone());
    }
    commit(tx, "reset git head", git)
}

/// Exports the bookmarks and tags to the Git store. A ref Git refuses
/// (a conflicted bookmark, a name Git cannot take) stays out, as the jj
/// CLI leaves it.
fn export(tx: &mut Transaction) -> Result<(), VcsError> {
    if get_git_backend(tx.repo().store()).is_err() {
        return Ok(());
    }
    export_refs(tx.repo_mut()).map_err(VcsError::ExportRefs)?;
    Ok(())
}

/// Points Git's `HEAD` in workspace `name` at `wc`'s parent and resets
/// the index to it, with new files in `wc` added as intent to add, so
/// `git status` there shows what `jj diff` does and `git ls-files`
/// lists them (jj-lib's `reset_head`). Only in a workspace colocated
/// with the Git store, as jj-cli asks
/// (`is_colocated_git_workspace`): the store's working tree, or one of
/// its worktrees. Elsewhere, and in a worktree too broken to open,
/// which the next open makes again, nothing happens.
fn keep_in_step(
    tx: &mut Transaction,
    name: &WorkspaceName,
    root: &Path,
    wc: &Commit,
) -> Result<(), VcsError> {
    let Ok(backend) = get_git_backend(tx.repo().store()) else {
        return Ok(());
    };
    if backend.open_git_repo_at_workdir(root).is_err() {
        return Ok(());
    }
    block_on(reset_head(tx.repo_mut(), name, root, wc))
        .map_err(VcsError::ResetHead)
}

/// Whether `a` and `b` are the same directory, by their canonical paths
/// when both exist.
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    let canonical = |path: &Path| -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
    };
    a == b || canonical(a) == canonical(b)
}
