//! What every tool does around its own work: snapshot the working copy,
//! run a transaction, check out the result (`docs/reference/vcs.md`,
//! "Scoping rules"). Runs inside a job on the workspace only.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use jj_lib::{
    backend::CommitId,
    commit::Commit,
    git::REMOTE_NAME_FOR_LOCAL_GIT_REPO,
    gitignore::GitIgnoreFile,
    matchers::{EverythingMatcher, Matcher, NothingMatcher, PrefixMatcher},
    merge::Merge,
    merged_tree::MergedTree,
    object_id::{HexPrefix, ObjectId as _, PrefixResolution},
    ref_name::WorkspaceName,
    repo::{ReadonlyRepo, Repo, RepoLoader},
    repo_path::RepoPathBuf,
    transaction::Transaction,
    working_copy::{SnapshotOptions, UntrackedReason, WorkingCopyFreshness},
    workspace::{LockedWorkspace, Workspace},
};
use pollster::block_on;

use crate::{TooLarge, error::VcsError, lock::lock_repo, vcs::Worker};

/// The operation attribute naming the tool that wrote an operation.
pub(crate) const TOOL_ATTRIBUTE: &str = "tau.vcs.tool";
/// The operation attribute an undo sets to the operation it undid.
pub(crate) const UNDO_ATTRIBUTE: &str = "tau.vcs.undo";
/// The operation attribute of a checkpoint that only moved the run's
/// bookmark: the turn changed nothing. Undo passes over it, as it does
/// over snapshots.
pub(crate) const BOOKMARK_ATTRIBUTE: &str = "tau.vcs.bookmark";
/// The operation attribute of a landing: the change ids it landed,
/// separated by spaces. A turn's paths leave them out.
pub(crate) const LANDED_ATTRIBUTE: &str = "tau.vcs.landed";
/// The operation attribute of a confirmed landing that says what it
/// did: `{ "child_head": <hex>, "landing": Landing }`, so a host that
/// closed before recording the landing can read it back
/// (`ProjectRepo::landed`).
pub(crate) const LANDING_ATTRIBUTE: &str = "tau.vcs.landing";
/// The operation attribute of a catch-up: the commit, in hex, a run moved
/// onto (`move_onto`, or a step that follows an update). A turn's paths
/// leave out what it brought, to that run and to the runs standing on it.
pub(crate) const CAUGHT_UP_ATTRIBUTE: &str = "tau.vcs.caught_up";

/// New files larger than this stay out of the snapshot, as jj's
/// default. Every other new file is tracked: nothing is staged.
pub const MAX_NEW_FILE_SIZE: u64 = 1024 * 1024;

/// How many hex digits of an id the tools show.
pub(crate) const SHORT_ID: usize = 12;

/// The repo after a snapshot, and the working-copy commit in it.
pub(crate) struct Snapshot {
    pub repo: Arc<ReadonlyRepo>,
    pub wc: Commit,
    /// New files over [`MAX_NEW_FILE_SIZE`], which the snapshot left
    /// out of `@`.
    pub too_large: Vec<TooLarge>,
}

/// Snapshots the working copy, one operation if files changed, and
/// returns the repo at head.
pub(crate) fn snapshot(worker: &mut Worker) -> Result<Snapshot, VcsError> {
    let workspace = worker.workspace()?;
    let _repo = lock_repo(workspace.repo_path())?;
    let name = workspace.workspace_name().to_owned();
    let loader = workspace.repo_loader().clone();
    let mut locked = block_on(workspace.start_working_copy_mutation())?;
    let snapshot = snapshot_locked(&mut locked, &loader, &name)?;
    block_on(locked.finish(snapshot.repo.op_id().clone()))?;
    Ok(snapshot)
}

/// Snapshots the working copy, then runs `edit` in one transaction
/// tagged with `tool`, and checks out the working-copy commit that
/// results. `edit` gets the working-copy commit after the snapshot.
pub(crate) fn mutate<T>(
    worker: &mut Worker,
    tool: &str,
    edit: impl FnOnce(&mut Transaction, &Commit) -> Result<T, VcsError>,
) -> Result<(Snapshot, T), VcsError> {
    let workspace = worker.workspace()?;
    let _repo = lock_repo(workspace.repo_path())?;
    let name = workspace.workspace_name().to_owned();
    let loader = workspace.repo_loader().clone();
    let mut locked = block_on(workspace.start_working_copy_mutation())?;
    let before = snapshot_locked(&mut locked, &loader, &name)?;
    let written = (|| {
        if is_immutable(before.repo.as_ref(), before.wc.id())? {
            return Err(VcsError::Immutable(short_commit(before.wc.id())));
        }
        let mut tx = before.repo.start_transaction();
        tx.set_workspace_name(&name);
        tx.set_attribute(TOOL_ATTRIBUTE.to_owned(), tool.to_owned());
        let value = edit(&mut tx, &before.wc)?;
        if tx.repo().has_rewrites() {
            block_on(tx.repo_mut().rebase_descendants())?;
        }
        let repo = if tx.repo().has_changes() {
            block_on(tx.commit(format!("tau vcs: {tool}")))?
        } else {
            before.repo.clone()
        };
        Ok((repo, value))
    })();
    // The snapshot stands when the tool fails: the working copy records
    // it, or the next tool would take what it wrote for edits since.
    let (repo, value) = match written {
        Ok(written) => written,
        Err(err) => {
            block_on(locked.finish(before.repo.op_id().clone()))?;
            return Err(err);
        }
    };
    let wc = wc_commit(&repo, &name)?;
    if wc.id() != before.wc.id() {
        block_on(locked.locked_wc().check_out(&wc))?;
    }
    block_on(locked.finish(repo.op_id().clone()))?;
    Ok((
        Snapshot {
            repo,
            wc,
            too_large: before.too_large,
        },
        value,
    ))
}

fn snapshot_locked(
    locked: &mut LockedWorkspace<'_>,
    loader: &RepoLoader,
    name: &WorkspaceName,
) -> Result<Snapshot, VcsError> {
    let options = SnapshotOptions {
        base_ignores: GitIgnoreFile::empty(),
        progress: None,
        start_tracking_matcher: &EverythingMatcher,
        force_tracking_matcher: &NothingMatcher,
        max_new_file_size: MAX_NEW_FILE_SIZE,
    };
    let mut repo = block_on(loader.load_at_head())?;
    let mut wc = wc_commit(&repo, name)?;
    // Two operations that rewrote `@` from the same one, merged when the
    // repository loaded: one side is a stray copy, and either may hold
    // the work. tau-vcs's own writes never do this (`crate::lock`);
    // another program's can.
    if divergent(&repo, &wc)? {
        return Err(VcsError::Stale);
    }
    match block_on(WorkingCopyFreshness::check_stale(
        locked.locked_wc(),
        &wc,
        &repo,
    ))? {
        WorkingCopyFreshness::Fresh => {}
        WorkingCopyFreshness::Updated(op) => {
            repo = block_on(repo.reload_at(&op))?;
            wc = wc_commit(&repo, name)?;
        }
        // Another workspace's operation rewrote this one's commit: the
        // main chat catching up with trunk restacks the commits a chat
        // stands on. The files move to it, as jj's `workspace
        // update-stale` does, with what was edited on disk since the last
        // snapshot merged on top, as a rebase would.
        WorkingCopyFreshness::WorkingCopyStale => {
            let before = locked.locked_wc().old_tree().clone();
            let (disk, _) = block_on(locked.locked_wc().snapshot(&options))?;
            if disk.tree_ids_and_labels() != before.tree_ids_and_labels() {
                let merged =
                    block_on(MergedTree::merge(Merge::from_vec(vec![
                        (wc.tree(), "the rewritten working copy".to_owned()),
                        (before, "the last snapshot".to_owned()),
                        (disk, "edits since".to_owned()),
                    ])))?;
                let mut tx = repo.start_transaction();
                tx.set_is_snapshot(true);
                tx.set_workspace_name(name);
                let edited = write_commit(&mut tx, |repo| {
                    repo.rewrite_commit(&wc).set_tree(merged.clone())
                })?;
                tx.repo_mut()
                    .set_wc_commit(name.to_owned(), edited.id().clone())?;
                block_on(tx.repo_mut().rebase_descendants())?;
                repo = block_on(tx.commit("snapshot working copy"))?;
                wc = edited;
            }
            block_on(locked.locked_wc().check_out(&wc))?;
        }
        WorkingCopyFreshness::SiblingOperation => {
            return Err(VcsError::Stale);
        }
    }

    let (tree, stats) = block_on(locked.locked_wc().snapshot(&options))?;
    // Every new file matches `start_tracking_matcher`, so size is the
    // only reason a file stays out.
    let too_large = stats
        .untracked_paths
        .iter()
        .filter_map(|(path, reason)| match reason {
            UntrackedReason::FileTooLarge { size, .. } => Some(TooLarge {
                path: path.as_internal_file_string().to_owned(),
                size: *size,
            }),
            UntrackedReason::FileNotAutoTracked => None,
        })
        .collect();

    if tree.tree_ids_and_labels() != wc.tree().tree_ids_and_labels() {
        let mut tx = repo.start_transaction();
        tx.set_is_snapshot(true);
        tx.set_workspace_name(name);
        // A snapshot can be the very commit an undo hid, as when a file
        // a restore deleted is deleted again once the restore is undone.
        let new_wc = write_commit(&mut tx, |repo| {
            repo.rewrite_commit(&wc).set_tree(tree.clone())
        })?;
        tx.repo_mut()
            .set_wc_commit(name.to_owned(), new_wc.id().clone())?;
        block_on(tx.repo_mut().rebase_descendants())?;
        repo = block_on(tx.commit("snapshot working copy"))?;
        wc = new_wc;
    }
    Ok(Snapshot {
        repo,
        wc,
        too_large,
    })
}

/// Whether other visible commits share `commit`'s change id.
fn divergent(repo: &ReadonlyRepo, commit: &Commit) -> Result<bool, VcsError> {
    let Some(targets) = block_on(repo.resolve_change_id(commit.change_id()))?
    else {
        return Ok(false);
    };
    Ok(targets.visible_with_offsets().nth(1).is_some())
}

/// Writes the commit `build` makes, with the committer's time moved on a
/// second when jj already has one with the same content.
///
/// Git stores commit times in whole seconds, so redoing a rewrite an
/// undo took back, within the same second, makes the very commit the
/// undo hid (a describe again, or a snapshot of the same files), and jj
/// refuses it ("Newly-created commit ... already exists"). jj's Git
/// backend nudges the time only when the change ids differ; this is the
/// same fix for the same change.
pub(crate) fn write_commit(
    tx: &mut Transaction,
    build: impl Fn(
        &mut jj_lib::repo::MutableRepo,
    ) -> jj_lib::commit_builder::CommitBuilder<'_>,
) -> Result<Commit, VcsError> {
    const TRIES: i64 = 16;
    for nudge in 0..TRIES {
        let mut builder = build(tx.repo_mut());
        if nudge > 0 {
            let mut committer = builder.committer().clone();
            committer.timestamp.timestamp.0 += nudge * 1000;
            builder = builder.set_committer(committer);
        }
        match block_on(builder.write()) {
            Err(err)
                if nudge + 1 < TRIES
                    && err.to_string().contains("already exists") => {}
            result => return Ok(result?),
        }
    }
    unreachable!("the last try returns")
}

/// The working-copy commit of workspace `name`.
pub(crate) fn wc_commit(
    repo: &Arc<ReadonlyRepo>,
    name: &WorkspaceName,
) -> Result<Commit, VcsError> {
    let id = repo
        .view()
        .get_wc_commit_id(name)
        .ok_or(VcsError::NoWorkingCopy)?;
    Ok(repo.store().get_commit(id)?)
}

/// Whether `id` is immutable: the root commit, or an ancestor of a tag
/// or a remote bookmark.
pub(crate) fn is_immutable(
    repo: &dyn Repo,
    id: &CommitId,
) -> Result<bool, VcsError> {
    if id == repo.store().root_commit_id() {
        return Ok(true);
    }
    let view = repo.view();
    let remote = view
        .all_remote_bookmarks()
        .filter(|(symbol, _)| symbol.remote != REMOTE_NAME_FOR_LOCAL_GIT_REPO)
        .flat_map(|(_, remote_ref)| remote_ref.target.added_ids());
    let tags = view.local_tags().flat_map(|(_, target)| target.added_ids());
    for head in remote.chain(tags) {
        if block_on(repo.index().is_ancestor(id, head))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Finds the commit a change id or a commit id (or a unique prefix of
/// either) names. Nothing else is accepted: no revsets, no names.
pub(crate) fn resolve(repo: &dyn Repo, rev: &str) -> Result<Commit, VcsError> {
    let rev = rev.trim();
    let not_an_id = || VcsError::NotAnId(rev.to_owned());
    if rev.is_empty() {
        return Err(not_an_id());
    }
    if rev.bytes().all(|b| (b'k'..=b'z').contains(&b)) {
        let prefix =
            HexPrefix::try_from_reverse_hex(rev).ok_or_else(not_an_id)?;
        return match block_on(repo.resolve_change_id_prefix(&prefix))? {
            PrefixResolution::NoMatch => {
                Err(VcsError::NoChange(rev.to_owned()))
            }
            PrefixResolution::AmbiguousMatch => {
                Err(VcsError::AmbiguousChange(rev.to_owned()))
            }
            PrefixResolution::SingleMatch(targets) => {
                let visible: Vec<&CommitId> =
                    targets.visible_with_offsets().map(|(_, id)| id).collect();
                match visible.as_slice() {
                    [] => Err(VcsError::Hidden(rev.to_owned())),
                    [id] => Ok(repo.store().get_commit(id)?),
                    _ => Err(VcsError::DivergentChange(rev.to_owned())),
                }
            }
        };
    }
    if rev
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        let prefix = HexPrefix::try_from_hex(rev).ok_or_else(not_an_id)?;
        return match block_on(repo.index().resolve_commit_id_prefix(&prefix))? {
            PrefixResolution::NoMatch => {
                Err(VcsError::NoCommit(rev.to_owned()))
            }
            PrefixResolution::AmbiguousMatch => {
                Err(VcsError::AmbiguousCommit(rev.to_owned()))
            }
            PrefixResolution::SingleMatch(id) => {
                Ok(repo.store().get_commit(&id)?)
            }
        };
    }
    Err(not_an_id())
}

/// A path the model gave, as a path in the repo. Relative paths are
/// relative to the workspace root; absolute ones must be inside it.
pub(crate) fn repo_path(
    workspace: &Workspace,
    given_root: &Path,
    input: &str,
) -> Result<RepoPathBuf, VcsError> {
    let path = Path::new(input.trim());
    let relative = if path.is_absolute() {
        path.strip_prefix(workspace.workspace_root())
            .or_else(|_| path.strip_prefix(given_root))
            .map_err(|_| VcsError::OutsideRepo(input.to_owned()))?
    } else {
        path
    };
    let cleaned: PathBuf = relative
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    if cleaned.as_os_str().is_empty() {
        return Ok(RepoPathBuf::root());
    }
    RepoPathBuf::from_relative_path(&cleaned)
        .map_err(|_| VcsError::NotInRepo(input.to_owned()))
}

/// Matches `paths` and everything under them; everything when there are
/// none.
pub(crate) fn matcher(paths: Vec<RepoPathBuf>) -> Box<dyn Matcher> {
    if paths.is_empty() {
        Box::new(EverythingMatcher)
    } else {
        Box::new(PrefixMatcher::new(paths))
    }
}

/// The first [`SHORT_ID`] digits of a commit id.
pub(crate) fn short_commit(id: &CommitId) -> String {
    let mut hex = id.hex();
    hex.truncate(SHORT_ID);
    hex
}
