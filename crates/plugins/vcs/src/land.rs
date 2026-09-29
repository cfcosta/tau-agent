//! Landing a child run on its parent (ADR 0009): the child's changes,
//! from where it started up to its head, are rebased onto the parent's
//! newest commit, keeping their change ids, and the parent's working
//! copy starts again on top. It runs in the parent's workspace, so the
//! parent's files follow in the same operation.

use std::collections::HashSet;

use futures_util::StreamExt as _;
use jj_lib::{
    backend::CommitId,
    commit::Commit,
    object_id::ObjectId as _,
    op_store::RefTarget,
    ref_name::RefName,
    repo::Repo,
    revset::ResolvedRevsetExpression,
    rewrite::rebase_commit,
    transaction::Transaction,
};
use pollster::block_on;
use serde::{Deserialize, Serialize};

use crate::{ChangeInfo, error::VcsError, session, vcs::Worker};

/// What landing a child did, or would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Landing {
    /// The child's changes as they are on the parent's stack after the
    /// landing, newest first. Empty when the child had nothing the
    /// parent lacks.
    pub changes: Vec<ChangeInfo>,
    /// Paths that hold conflict markers in the parent's new newest
    /// commit: what the parent's next turn has to resolve.
    pub conflicts: Vec<String>,
    /// The parent's newest commit after the landing, in hex.
    pub head: String,
}

/// Lands the child whose newest commit is `child_head` (a full commit
/// id in hex) on this workspace's run, and points `bookmark`, the
/// parent's, at the new head. With `confirm` off, nothing changes: the
/// result says what landing would do, conflicts included.
pub(crate) fn land(
    worker: &mut Worker,
    child_head: &str,
    bookmark: &str,
    confirm: bool,
) -> Result<Landing, VcsError> {
    let child_head = CommitId::try_from_hex(child_head)
        .ok_or_else(|| VcsError::NotCommitId(child_head.to_owned()))?;
    let name = worker.workspace()?.workspace_name().to_owned();
    if confirm {
        let (_, landing) = session::mutate(worker, "land", |tx, wc| {
            restack(tx, wc, &name, &child_head, bookmark)
        })?;
        return Ok(landing);
    }
    // A preview: the same rewrite in a transaction that is dropped.
    let snapshot = session::snapshot(worker)?;
    let mut tx = snapshot.repo.start_transaction();
    restack(&mut tx, &snapshot.wc, &name, &child_head, bookmark)
}

fn restack(
    tx: &mut Transaction,
    wc: &Commit,
    workspace: &jj_lib::ref_name::WorkspaceName,
    child_head: &CommitId,
    bookmark: &str,
) -> Result<Landing, VcsError> {
    // Landing happens between turns, when the parent's work is all
    // committed; anything in its working copy would be buried.
    if !block_on(wc.is_empty(tx.repo()))? {
        return Err(VcsError::ParentChanged);
    }
    let head = match wc.parent_ids() {
        [head] => head.clone(),
        _ => return Err(VcsError::ParentMerge),
    };

    // The child's changes: what its head has that the parent's lacks.
    let moving: Vec<CommitId> = {
        let revset = ResolvedRevsetExpression::commit(child_head.clone())
            .ancestors()
            .minus(&ResolvedRevsetExpression::commit(head.clone()).ancestors())
            .evaluate(tx.repo())?;
        block_on(revset.stream().collect::<Vec<_>>())
            .into_iter()
            .collect::<Result<_, _>>()?
    };
    if moving.is_empty() {
        let head_commit = tx.repo().store().get_commit(&head)?;
        return Ok(Landing {
            changes: Vec::new(),
            conflicts: conflicts(&head_commit),
            head: head.hex(),
        });
    }
    let set: HashSet<&CommitId> = moving.iter().collect();

    // Rebase the child's roots (the changes whose parents are not the
    // child's) onto the parent's head; their descendants follow.
    let mut roots = Vec::new();
    for id in moving.iter().rev() {
        let commit = tx.repo().store().get_commit(id)?;
        if commit
            .parent_ids()
            .iter()
            .all(|parent| !set.contains(parent))
        {
            roots.push(commit);
        }
    }
    if roots.len() > 1 {
        return Err(VcsError::NotOneStack);
    }
    for root in roots {
        // A child the parent waited on already sits on the parent's
        // head: nothing to rewrite.
        if root.parent_ids() == std::slice::from_ref(&head) {
            continue;
        }
        block_on(rebase_commit(tx.repo_mut(), root, vec![head.clone()]))?;
    }
    block_on(tx.repo_mut().rebase_descendants())?;

    // The child's head, where it is now: its change id names it.
    let old_head = tx.repo().store().get_commit(child_head)?;
    let new_head = current(tx.repo(), &old_head)?;
    block_on(tx.repo_mut().check_out(workspace.to_owned(), &new_head))?;
    tx.repo_mut().set_local_bookmark_target(
        RefName::new(bookmark),
        RefTarget::normal(new_head.id().clone()),
    );

    let wc_id = tx
        .repo()
        .view()
        .get_wc_commit_id(workspace)
        .cloned()
        .ok_or(VcsError::NoWorkingCopy)?;
    let mut changes = Vec::new();
    for id in &moving {
        let old = tx.repo().store().get_commit(id)?;
        let new = current(tx.repo(), &old)?;
        changes.push(ChangeInfo::of(tx.repo(), &new, &wc_id)?);
    }
    Ok(Landing {
        changes,
        conflicts: conflicts(&new_head),
        head: new_head.id().hex(),
    })
}

/// The visible commit `commit`'s change names now.
fn current(repo: &dyn Repo, commit: &Commit) -> Result<Commit, VcsError> {
    let targets = block_on(repo.resolve_change_id(commit.change_id()))?
        .ok_or(VcsError::LandedMissing)?;
    let visible: Vec<&CommitId> =
        targets.visible_with_offsets().map(|(_, id)| id).collect();
    match visible.as_slice() {
        [id] => Ok(repo.store().get_commit(id)?),
        _ => Err(VcsError::DivergentAfterLanding(
            commit.change_id().reverse_hex(),
        )),
    }
}

fn conflicts(commit: &Commit) -> Vec<String> {
    commit
        .tree()
        .conflicts()
        .map(|(path, _)| path.as_internal_file_string().to_owned())
        .collect()
}
