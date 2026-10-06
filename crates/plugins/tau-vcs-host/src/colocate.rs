//! A project's Git store is colocated with its main workspace, as
//! `jj git init --colocate` makes one: `main/.git` beside `main/.jj`.
//! The jj CLI keeps such a store in step after each command; jj-lib
//! leaves that to its caller, so every operation tau writes goes
//! through [`commit`], which does the same: bookmarks and tags go out
//! as Git's branches and tags, and Git's `HEAD` and index follow the
//! main workspace's working copy, so `git` in `main/` sees what jj does.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{path::Path, sync::Arc};

use jj_lib::{
    git::{export_refs, get_git_backend, reset_head},
    ref_name::WorkspaceName,
    repo::{ReadonlyRepo, Repo as _},
    transaction::Transaction,
};
use pollster::block_on;

use crate::error::VcsError;

/// Keeps `.jj/` out of Git's view in the colocated `workspace_root`, as
/// the jj CLI does: a `.jj/.gitignore` that ignores all it holds.
/// jj-lib writes none, and without it `git status` lists `.jj/` as
/// untracked.
pub(crate) fn ignore_jj_dir(workspace_root: &Path) -> Result<(), VcsError> {
    let path = workspace_root.join(".jj").join(".gitignore");
    std::fs::write(&path, "/*\n")
        .map_err(|source| VcsError::Create { path, source })
}

/// Commits `tx` as `description`, with the Git store brought in step
/// first.
pub(crate) fn commit(
    mut tx: Transaction,
    description: impl Into<String>,
) -> Result<Arc<ReadonlyRepo>, VcsError> {
    sync_git(&mut tx)?;
    Ok(block_on(tx.commit(description))?)
}

/// Exports the bookmarks and tags to the Git store, and, when the store
/// has a working tree and the main workspace's working copy changed,
/// points Git's `HEAD` at its parent and resets the index to it. A ref
/// Git refuses (a conflicted bookmark, a name Git cannot take) stays
/// out, as the jj CLI leaves it.
fn sync_git(tx: &mut Transaction) -> Result<(), VcsError> {
    let Ok(backend) = get_git_backend(tx.repo().store()) else {
        return Ok(());
    };
    let workdir = backend.git_repo().workdir().map(Path::to_owned);
    export_refs(tx.repo_mut()).map_err(VcsError::ExportRefs)?;
    let Some(workdir) = workdir else {
        return Ok(());
    };
    let main = WorkspaceName::DEFAULT;
    let Some(wc) = tx.repo().view().get_wc_commit_id(main).cloned() else {
        return Ok(());
    };
    if tx.base_repo().view().get_wc_commit_id(main) == Some(&wc) {
        return Ok(());
    }
    let wc = tx.repo().store().get_commit(&wc)?;
    block_on(reset_head(tx.repo_mut(), main, &workdir, &wc))
        .map_err(VcsError::ResetHead)
}
