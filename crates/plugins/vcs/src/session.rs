//! What every tool does around its own work: snapshot the working copy,
//! run a transaction, check out the result (`docs/reference/vcs.md`,
//! "Scoping rules"). Runs on the workspace's thread only.

use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, bail};
use jj_lib::{
    backend::CommitId,
    commit::Commit,
    git::REMOTE_NAME_FOR_LOCAL_GIT_REPO,
    gitignore::GitIgnoreFile,
    matchers::{EverythingMatcher, Matcher, NothingMatcher, PrefixMatcher},
    object_id::{HexPrefix, ObjectId as _, PrefixResolution},
    ref_name::WorkspaceName,
    repo::{ReadonlyRepo, Repo, RepoLoader},
    repo_path::RepoPathBuf,
    transaction::Transaction,
    working_copy::{SnapshotOptions, UntrackedReason, WorkingCopyFreshness},
    workspace::{LockedWorkspace, Workspace},
};
use pollster::block_on;
use serde::{Deserialize, Serialize};

use crate::vcs::Worker;

/// The operation attribute naming the tool that wrote an operation.
pub(crate) const TOOL_ATTRIBUTE: &str = "tau.vcs.tool";
/// The operation attribute an undo sets to the operation it undid.
pub(crate) const UNDO_ATTRIBUTE: &str = "tau.vcs.undo";

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

/// A new file the snapshot left out of `@` for its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TooLarge {
    pub path: String,
    /// Its size in bytes.
    pub size: u64,
}

/// Snapshots the working copy, one operation if files changed, and
/// returns the repo at head.
pub(crate) fn snapshot(worker: &mut Worker) -> anyhow::Result<Snapshot> {
    let workspace = worker.workspace()?;
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
    edit: impl FnOnce(&mut Transaction, &Commit) -> anyhow::Result<T>,
) -> anyhow::Result<(Snapshot, T)> {
    let workspace = worker.workspace()?;
    let name = workspace.workspace_name().to_owned();
    let loader = workspace.repo_loader().clone();
    let mut locked = block_on(workspace.start_working_copy_mutation())?;
    let before = snapshot_locked(&mut locked, &loader, &name)?;
    if is_immutable(before.repo.as_ref(), before.wc.id())? {
        bail!(
            "The working-copy commit {} is immutable",
            short_commit(before.wc.id())
        );
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
) -> anyhow::Result<Snapshot> {
    let mut repo = block_on(loader.load_at_head())?;
    let mut wc = wc_commit(&repo, name)?;
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
        WorkingCopyFreshness::WorkingCopyStale
        | WorkingCopyFreshness::SiblingOperation => bail!(
            "The working copy is stale: another process changed this \
             workspace's commit. Ask the user to update the workspace."
        ),
    }

    let options = SnapshotOptions {
        base_ignores: GitIgnoreFile::empty(),
        progress: None,
        start_tracking_matcher: &EverythingMatcher,
        force_tracking_matcher: &NothingMatcher,
        max_new_file_size: MAX_NEW_FILE_SIZE,
    };
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
        let new_wc =
            block_on(tx.repo_mut().rewrite_commit(&wc).set_tree(tree).write())?;
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

/// Writes the commit `build` makes, with the committer's time moved on a
/// second when jj already has one with the same content.
///
/// Git stores commit times in whole seconds, so redoing a rewrite an
/// undo took back, within the same second, makes the very commit the
/// undo hid, and jj refuses it ("Newly-created commit ... already
/// exists"). jj's Git backend nudges the time only when the change ids
/// differ; this is the same fix for the same change.
pub(crate) fn write_commit(
    tx: &mut Transaction,
    build: impl Fn(
        &mut jj_lib::repo::MutableRepo,
    ) -> jj_lib::commit_builder::CommitBuilder<'_>,
) -> anyhow::Result<Commit> {
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
) -> anyhow::Result<Commit> {
    let id = repo
        .view()
        .get_wc_commit_id(name)
        .ok_or_else(|| anyhow!("The workspace has no working-copy commit"))?;
    Ok(repo.store().get_commit(id)?)
}

/// Whether `id` is immutable: the root commit, or an ancestor of a tag
/// or a remote bookmark.
pub(crate) fn is_immutable(
    repo: &dyn Repo,
    id: &CommitId,
) -> anyhow::Result<bool> {
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
pub(crate) fn resolve(repo: &dyn Repo, rev: &str) -> anyhow::Result<Commit> {
    let rev = rev.trim();
    let not_an_id = || {
        anyhow!(
            "`{rev}` is not a change id or a commit id. Pass an id (or a \
             unique prefix) from vcs_log or vcs_status; revsets are not \
             accepted."
        )
    };
    if rev.is_empty() {
        return Err(not_an_id());
    }
    if rev.bytes().all(|b| (b'k'..=b'z').contains(&b)) {
        let prefix =
            HexPrefix::try_from_reverse_hex(rev).ok_or_else(not_an_id)?;
        return match block_on(repo.resolve_change_id_prefix(&prefix))? {
            PrefixResolution::NoMatch => bail!("No change matches `{rev}`"),
            PrefixResolution::AmbiguousMatch => {
                bail!("Change id prefix `{rev}` is ambiguous; give more of it")
            }
            PrefixResolution::SingleMatch(targets) => {
                let visible: Vec<&CommitId> =
                    targets.visible_with_offsets().map(|(_, id)| id).collect();
                match visible.as_slice() {
                    [] => bail!("Change `{rev}` is hidden (abandoned)"),
                    [id] => Ok(repo.store().get_commit(id)?),
                    _ => bail!(
                        "Change `{rev}` is divergent; pass a commit id instead"
                    ),
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
            PrefixResolution::NoMatch => bail!("No commit matches `{rev}`"),
            PrefixResolution::AmbiguousMatch => {
                bail!("Commit id prefix `{rev}` is ambiguous; give more of it")
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
) -> anyhow::Result<RepoPathBuf> {
    let path = Path::new(input.trim());
    let relative = if path.is_absolute() {
        path.strip_prefix(workspace.workspace_root())
            .or_else(|_| path.strip_prefix(given_root))
            .map_err(|_| anyhow!("`{input}` is outside the repository"))?
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
        .map_err(|_| anyhow!("`{input}` is not a path inside the repository"))
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
