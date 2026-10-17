//! A project's Git store is colocated with its main workspace, as
//! `jj git init --colocate` makes one: `main/.git` beside `main/.jj`.
//! The jj CLI keeps such a store in step after each command; jj-lib
//! leaves that to its caller, so every operation tau writes goes
//! through [`commit`], which does the same: bookmarks and tags go out
//! as Git's branches and tags, and Git's `HEAD` and index follow the
//! working copy of the workspace in the Git store's working tree, the
//! main chat's, so `git` in `main/` sees what jj does.
//! It also tags the operation as tau's ([`crate::operations`]).

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use jj_lib::{
    backend::CommitId,
    git::{export_refs, get_git_backend, reset_head},
    ref_name::{WorkspaceName, WorkspaceNameBuf},
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
/// Git store brought in step first. `_git` is jj-cli's Git
/// import/export lock, held from before any import the transaction
/// made until the operation is written, as the CLI holds it.
pub(crate) fn commit(
    mut tx: Transaction,
    what: impl AsRef<str>,
    _git: &GitLock,
) -> Result<Arc<ReadonlyRepo>, VcsError> {
    let description = crate::operations::tag(&mut tx, what.as_ref());
    sync_git(&mut tx)?;
    Ok(block_on(tx.commit(description))?)
}

/// Exports the bookmarks and tags to the Git store, and, when the store
/// has a working tree and the working copy of the workspace there
/// moved, points Git's `HEAD` at its parent and resets the index to it.
/// A ref Git refuses (a conflicted bookmark, a name Git cannot take)
/// stays out, as the jj CLI leaves it.
fn sync_git(tx: &mut Transaction) -> Result<(), VcsError> {
    let Ok(backend) = get_git_backend(tx.repo().store()) else {
        return Ok(());
    };
    let workdir = backend.git_repo().workdir().map(Path::to_owned);
    export_refs(tx.repo_mut()).map_err(VcsError::ExportRefs)?;
    let Some(workdir) = workdir else {
        return Ok(());
    };
    let Some(name) = workspace_at(tx, &workdir)? else {
        return Ok(());
    };
    let Some(wc) = tx.repo().view().get_wc_commit_id(&name).cloned() else {
        return Ok(());
    };
    if !moved(tx, &name, &wc) {
        return Ok(());
    }
    let wc = tx.repo().store().get_commit(&wc)?;
    block_on(reset_head(tx.repo_mut(), &name, &workdir, &wc))
        .map_err(VcsError::ResetHead)
}

/// The workspace of `tx`'s view whose directory is `dir`, as the
/// repository's workspace store has it: in a project, the main chat's
/// is the Git store's working tree.
fn workspace_at(
    tx: &Transaction,
    dir: &Path,
) -> Result<Option<WorkspaceNameBuf>, VcsError> {
    let store = tx.base_repo().loader().workspace_store();
    for name in tx.repo().view().wc_commit_ids().keys() {
        if let Some(path) = store.get_workspace_path(name)?
            && same_dir(&path, dir)
        {
            return Ok(Some(name.clone()));
        }
    }
    Ok(None)
}

/// Whether workspace `name`'s working copy, `wc` now, is not the one it
/// had before `tx`. A workspace renamed in `tx` keeps its commit under
/// its new name, and did not move: the main chat's, when tau names it
/// (`crate::project`).
fn moved(tx: &Transaction, name: &WorkspaceName, wc: &CommitId) -> bool {
    let before = tx.base_repo().view();
    if let Some(was) = before.get_wc_commit_id(name) {
        return was != wc;
    }
    let now = tx.repo().view();
    !before
        .wc_commit_ids()
        .iter()
        .any(|(old, was)| was == wc && now.get_wc_commit_id(old).is_none())
}

/// Whether `a` and `b` are the same directory, by their canonical paths
/// when both exist.
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    let canonical = |path: &Path| -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
    };
    a == b || canonical(a) == canonical(b)
}
