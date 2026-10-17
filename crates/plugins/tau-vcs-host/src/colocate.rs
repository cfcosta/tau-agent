//! A project's Git store is colocated with its main workspace, as
//! `jj git init --colocate` makes one: `main/.git` beside `main/.jj`,
//! and each run's workspace is a worktree of it ([`crate::worktree`]).
//! The jj CLI keeps such a store in step after each command; jj-lib
//! leaves that to its caller, so every operation tau writes goes
//! through [`commit`] or [`commit_in`], which do the same: bookmarks
//! and tags go out as Git's branches and tags, and Git's `HEAD` and
//! index follow the working copy of the workspace the operation was
//! written in, so `git` there sees what jj does. They also tag the
//! operation as tau's ([`crate::operations`]). In a local repository
//! (ADR 0043), tau's writes import Git's refs first ([`import_git`]),
//! and the person's checkout, the Git store's own working tree, never
//! has its `HEAD` or index moved.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use jj_lib::{
    commit::Commit,
    git::{
        GitImportOptions,
        GitSettings,
        export_refs,
        get_git_backend,
        import_refs,
        reset_head,
    },
    ref_name::WorkspaceName,
    repo::{ReadonlyRepo, Repo as _},
    revset::{ResolvedRevsetExpression, RevsetDiagnostics},
    rewrite::RebaseOptions,
    settings::UserSettings,
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
    if backend.open_git_repo_at_workdir(root).is_err()
        || is_persons_checkout(backend.git_repo().workdir(), root)
    {
        return Ok(());
    }
    block_on(reset_head(tx.repo_mut(), name, root, wc))
        .map_err(VcsError::ResetHead)
}

/// Whether `root` is the person's own checkout in a local repository
/// (ADR 0043): the Git store's working tree (`workdir`), with tau's
/// state beside its jj repository (`crate::local`). tau never moves
/// Git's `HEAD` or index there; its own workspaces are worktrees.
fn is_persons_checkout(workdir: Option<&Path>, root: &Path) -> bool {
    workdir.is_some_and(|workdir| {
        same_dir(workdir, root)
            && crate::local::is_local(&workdir.join(".jj").join("repo"))
    })
}

/// The options the jj CLI imports Git's refs with under `settings`
/// (jj-cli 0.46.0's `load_git_import_options`): the person's
/// `git.abandon-unreachable-commits` and
/// `git.record-synthetic-predecessors`, and each remote's
/// `remotes.<name>.auto-track-bookmarks`, the bookmarks a new
/// remote-tracking branch makes a local bookmark for.
pub(crate) fn import_options(
    settings: &UserSettings,
) -> Result<GitImportOptions, VcsError> {
    let git = GitSettings::from_settings(settings)?;
    let mut auto_track = HashMap::new();
    for (remote, remote_settings) in settings.remote_settings()? {
        let Some(text) = &remote_settings.auto_track_bookmarks else {
            continue;
        };
        let expression = jj_lib::revset::parse_string_expression(
            &mut RevsetDiagnostics::new(),
            text,
        )
        .map_err(|error| VcsError::ConfigRevset {
            name: format!(
                "remotes.{}.auto-track-bookmarks",
                remote.as_symbol()
            ),
            error: error.kind().to_string(),
        })?;
        auto_track.insert(remote, expression.to_matcher());
    }
    Ok(GitImportOptions {
        abandon_unreachable_commits: git.abandon_unreachable_commits,
        record_synthetic_predecessors: git.record_synthetic_predecessors,
        remote_auto_track_bookmarks: auto_track,
    })
}

/// Imports Git's branches and tags into `tx`, as the jj CLI does in a
/// colocated workspace before each command (jj-cli 0.46.0's
/// `import_git_refs`), so what the person did with `git` alone is in
/// the operation tau writes (ADR 0043): with `options`
/// ([`import_options`]), then what stood on commits Git rewrote or
/// abandoned rebased, except what is `immutable` (the immutable
/// commits of the repository `tx` started from, as jj-cli's
/// `rebase_mutable_descendants` takes them). `_git` is jj-cli's Git
/// import/export lock, held until the operation is written.
pub(crate) fn import_git(
    tx: &mut Transaction,
    options: &GitImportOptions,
    immutable: &Arc<ResolvedRevsetExpression>,
    _git: &GitLock,
) -> Result<(), VcsError> {
    block_on(import_refs(tx.repo_mut(), options))
        .map_err(VcsError::ImportBranches)?;
    if tx.repo().has_rewrites() {
        block_on(tx.repo_mut().rebase_descendants_with_options(
            immutable,
            &RebaseOptions::default(),
            |_, _| {},
        ))?;
    }
    Ok(())
}

/// Whether `a` and `b` are the same directory, by their canonical paths
/// when both exist.
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    let canonical = |path: &Path| -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
    };
    a == b || canonical(a) == canonical(b)
}
