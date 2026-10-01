//! Landing a child run on its parent (ADR 0009): the child's changes,
//! from where it started up to its head, are rebased onto the parent's
//! newest commit, keeping their change ids, and the parent's working
//! copy, with whatever it holds, moves on top (ADR 0014). It runs in the
//! parent's workspace, so the parent's files follow in the same
//! operation.
//!
//! Catching up with trunk moves the other way: a run's own changes go
//! onto trunk's head ([`move_onto`]), in the run's workspace, as the
//! main chat does when an update moved trunk without it.

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
            let landing = restack(tx, wc, &name, &child_head, bookmark)?;
            let landed: Vec<&str> = landing
                .changes
                .iter()
                .map(|change| change.change_id.as_str())
                .collect();
            tx.set_attribute(
                session::LANDED_ATTRIBUTE.to_owned(),
                landed.join(" "),
            );
            Ok(landing)
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
    // The parent's uncommitted work stays in its working copy, which
    // moves onto the landed changes.
    let dirty = !block_on(wc.is_empty(tx.repo()))?;
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
    if dirty {
        let wc = current(tx.repo(), wc)?;
        block_on(rebase_commit(
            tx.repo_mut(),
            wc,
            vec![new_head.id().clone()],
        ))?;
        block_on(tx.repo_mut().rebase_descendants())?;
    } else {
        block_on(tx.repo_mut().check_out(workspace.to_owned(), &new_head))?;
    }
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

/// Moves this workspace's run, its changes up to `@`, onto `onto` (a
/// full commit id in hex), and points `bookmark` at its newest commit
/// there. With `confirm` off, nothing changes.
///
/// The run's changes are what `@` has that neither `onto` nor the
/// commit it last moved onto has. That commit is upstream's, as trunk
/// was then, so upstream's commits under the run are never the run's,
/// even once upstream drops them (a reset, an amend, a force-push):
/// they stay dropped. The workspace records it in `.jj/tau-moved-onto`
/// after each move, unless `onto` is one of the run's own commits.
pub(crate) fn move_onto(
    worker: &mut Worker,
    onto: &str,
    bookmark: &str,
    confirm: bool,
) -> Result<Landing, VcsError> {
    let onto = CommitId::try_from_hex(onto)
        .ok_or_else(|| VcsError::NotCommitId(onto.to_owned()))?;
    let workspace = worker.workspace()?;
    let name = workspace.workspace_name().to_owned();
    let record = workspace.workspace_root().join(".jj").join(MOVED_ONTO);
    let last = std::fs::read_to_string(&record)
        .ok()
        .and_then(|hex| CommitId::try_from_hex(hex.trim()));
    if confirm {
        let mut onto_is_own = false;
        let (_, moved) = session::mutate(worker, "move_onto", |tx, wc| {
            onto_is_own = is_own(tx.repo(), wc, last.as_ref(), &onto)?;
            rebase_run(tx, wc, &name, &onto, last.as_ref(), bookmark)
        })?;
        if !onto_is_own {
            std::fs::write(&record, onto.hex())?;
        }
        return Ok(moved);
    }
    let snapshot = session::snapshot(worker)?;
    let mut tx = snapshot.repo.start_transaction();
    rebase_run(&mut tx, &snapshot.wc, &name, &onto, last.as_ref(), bookmark)
}

/// Where a workspace keeps the commit it last moved onto, below `.jj`.
const MOVED_ONTO: &str = "tau-moved-onto";

/// Whether `onto` is one of the run's own commits: under `wc`, and not
/// under `last`, the commit the run last moved onto.
fn is_own(
    repo: &dyn Repo,
    wc: &Commit,
    last: Option<&CommitId>,
    onto: &CommitId,
) -> Result<bool, VcsError> {
    let index = repo.index();
    if !block_on(index.is_ancestor(onto, wc.id()))? {
        return Ok(false);
    }
    Ok(match last.filter(|last| known(repo, last)) {
        Some(last) => !block_on(index.is_ancestor(onto, last))?,
        None => true,
    })
}

/// Whether the repository has `id`: a recorded commit may be from
/// before the repository was made again.
fn known(repo: &dyn Repo, id: &CommitId) -> bool {
    repo.store().get_commit(id).is_ok()
        && block_on(repo.index().has_id(id)).unwrap_or(false)
}

fn rebase_run(
    tx: &mut Transaction,
    wc: &Commit,
    workspace: &jj_lib::ref_name::WorkspaceName,
    onto: &CommitId,
    last: Option<&CommitId>,
    bookmark: &str,
) -> Result<Landing, VcsError> {
    let moving: Vec<CommitId> = {
        let mut upstream = ResolvedRevsetExpression::commit(onto.clone());
        if let Some(last) = last.filter(|last| known(tx.repo(), last)) {
            upstream =
                upstream.union(&ResolvedRevsetExpression::commit(last.clone()));
        }
        let revset = ResolvedRevsetExpression::commit(wc.id().clone())
            .ancestors()
            .minus(&upstream.ancestors())
            .evaluate(tx.repo())?;
        block_on(revset.stream().collect::<Vec<_>>())
            .into_iter()
            .collect::<Result<_, _>>()?
    };
    let set: HashSet<&CommitId> = moving.iter().collect();
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
        if root.parent_ids() == std::slice::from_ref(onto) {
            continue;
        }
        block_on(rebase_commit(tx.repo_mut(), root, vec![onto.clone()]))?;
    }
    block_on(tx.repo_mut().rebase_descendants())?;

    let wc_id = tx
        .repo()
        .view()
        .get_wc_commit_id(workspace)
        .cloned()
        .ok_or(VcsError::NoWorkingCopy)?;
    let new_wc = tx.repo().store().get_commit(&wc_id)?;
    let head = match new_wc.parent_ids() {
        [head] => tx.repo().store().get_commit(head)?,
        _ => return Err(VcsError::ParentMerge),
    };
    tx.repo_mut().set_local_bookmark_target(
        RefName::new(bookmark),
        RefTarget::normal(head.id().clone()),
    );
    let mut changes = Vec::new();
    for id in &moving {
        if id == wc.id() {
            continue;
        }
        let old = tx.repo().store().get_commit(id)?;
        let new = current(tx.repo(), &old)?;
        changes.push(ChangeInfo::of(tx.repo(), &new, &wc_id)?);
    }
    // The run's own work in `@` can conflict too.
    let mut found = conflicts(&head);
    for path in conflicts(&new_wc) {
        if !found.contains(&path) {
            found.push(path);
        }
    }
    Ok(Landing {
        changes,
        conflicts: found,
        head: head.id().hex(),
    })
}
