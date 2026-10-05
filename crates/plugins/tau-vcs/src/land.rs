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

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0027)"
)]

use std::collections::HashSet;

use futures_util::StreamExt as _;
use jj_lib::{
    backend::CommitId,
    commit::Commit,
    git::REMOTE_NAME_FOR_LOCAL_GIT_REPO,
    object_id::ObjectId as _,
    op_store::RefTarget,
    ref_name::RefName,
    repo::Repo,
    revset::ResolvedRevsetExpression,
    rewrite::rebase_commit,
    transaction::Transaction,
};
use pollster::block_on;

use crate::{ChangeInfo, Landing, error::VcsError, session, vcs::Worker};

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
    let record = MovedOnto::load(worker)?;
    if confirm {
        let mut followed = None;
        let (_, landing) = session::mutate(worker, "land", |tx, wc| {
            let (wc, onto) =
                follow_bookmark(tx, wc, &name, bookmark, record.last())?;
            followed = onto;
            let landing = restack(tx, &wc, &name, &child_head, bookmark)?;
            let landed: Vec<&str> = landing
                .changes
                .iter()
                .map(|change| change.change_id.as_str())
                .collect();
            tx.set_attribute(
                session::LANDED_ATTRIBUTE.to_owned(),
                landed.join(" "),
            );
            let record = serde_json::json!({
                "child_head": child_head.hex(),
                "landing": &landing,
            });
            tx.set_attribute(
                session::LANDING_ATTRIBUTE.to_owned(),
                record.to_string(),
            );
            Ok(landing)
        })?;
        record.save(followed.as_ref())?;
        return Ok(landing);
    }
    // A preview: the same rewrite in a transaction that is dropped.
    let snapshot = session::snapshot(worker)?;
    let mut tx = snapshot.repo.start_transaction();
    let (wc, _) =
        follow_bookmark(&mut tx, &snapshot.wc, &name, bookmark, record.last())?;
    restack(&mut tx, &wc, &name, &child_head, bookmark)
}

/// Keeps `bookmark` moving forward after an update: when an update moved
/// it to upstream's commit (see [`upstream`]) and the run's newest
/// commit (`wc`'s parent) does not have that commit, as trunk's after an
/// update while the main chat ran, the run's changes, up to `@`, move
/// onto it first, as a catch-up does (`move_onto`). Returns `@` after,
/// and the commit it moved onto, for [`MovedOnto::save`].
pub(crate) fn follow_bookmark(
    tx: &mut Transaction,
    wc: &Commit,
    workspace: &jj_lib::ref_name::WorkspaceName,
    bookmark: &str,
    last: Option<&CommitId>,
) -> Result<(Commit, Option<CommitId>), VcsError> {
    let Some(target) = upstream(tx, bookmark) else {
        return Ok((wc.clone(), None));
    };
    let [head] = wc.parent_ids() else {
        return Ok((wc.clone(), None));
    };
    if block_on(tx.repo().index().is_ancestor(&target, head))? {
        return Ok((wc.clone(), None));
    }
    rebase_run(tx, wc, workspace, &target, last, bookmark)?;
    tx.set_attribute(session::CAUGHT_UP_ATTRIBUTE.to_owned(), target.hex());
    let id = tx
        .repo()
        .view()
        .get_wc_commit_id(workspace)
        .cloned()
        .ok_or(VcsError::NoWorkingCopy)?;
    Ok((tx.repo().store().get_commit(&id)?, Some(target)))
}

/// `bookmark`'s commit when an update put it there: the source's branch
/// of that name, as `<bookmark>@git` names it. Only an update moves a
/// bookmark to upstream's commit; a run's bookmarks (`tau/<run>`) have
/// no such branch, and a bookmark a run moved names its own commit.
fn upstream(tx: &Transaction, bookmark: &str) -> Option<CommitId> {
    let name = RefName::new(bookmark);
    let view = tx.repo().view();
    let local = view.get_local_bookmark(name).as_normal()?;
    let remote = view
        .get_remote_bookmark(
            name.to_remote_symbol(REMOTE_NAME_FOR_LOCAL_GIT_REPO),
        )
        .target
        .as_normal()?;
    (local == remote).then(|| local.clone())
}

fn restack(
    tx: &mut Transaction,
    wc: &Commit,
    workspace: &jj_lib::ref_name::WorkspaceName,
    child_head: &CommitId,
    bookmark: &str,
) -> Result<Landing, VcsError> {
    // A head the caller read before something rewrote it, as the main
    // chat's catch-up restacks the chats on its commits: landing it
    // would bring the old copies back beside the new ones.
    let child = tx.repo().store().get_commit(child_head)?;
    if !visible(tx.repo(), &child)? {
        return Err(VcsError::HiddenHead(child_head.hex()));
    }
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

/// Whether `commit` is visible: its change names it.
fn visible(repo: &dyn Repo, commit: &Commit) -> Result<bool, VcsError> {
    let Some(targets) = block_on(repo.resolve_change_id(commit.change_id()))?
    else {
        return Ok(false);
    };
    Ok(targets
        .visible_with_offsets()
        .any(|(_, id)| id == commit.id()))
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
/// there, or removes it when that is the root commit. With `confirm`
/// off, nothing changes.
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
    let name = worker.workspace()?.workspace_name().to_owned();
    let record = MovedOnto::load(worker)?;
    let last = record.last().cloned();
    if confirm {
        // Where the run went: past `onto` when an update moved the
        // bookmark on meanwhile; recorded unless it is the run's own.
        let mut moved_onto = None;
        let (_, moved) = session::mutate(worker, "move_onto", |tx, wc| {
            let onto = past(tx, wc, &onto, bookmark)?;
            if !is_own(tx.repo(), wc, last.as_ref(), &onto)? {
                moved_onto = Some(onto.clone());
            }
            tx.set_attribute(
                session::CAUGHT_UP_ATTRIBUTE.to_owned(),
                onto.hex(),
            );
            rebase_run(tx, wc, &name, &onto, last.as_ref(), bookmark)
        })?;
        record.save(moved_onto.as_ref())?;
        return Ok(moved);
    }
    let snapshot = session::snapshot(worker)?;
    let mut tx = snapshot.repo.start_transaction();
    let onto = past(&tx, &snapshot.wc, &onto, bookmark)?;
    rebase_run(&mut tx, &snapshot.wc, &name, &onto, last.as_ref(), bookmark)
}

/// Where a workspace keeps the commit it last moved onto, below `.jj`.
const MOVED_ONTO: &str = "tau-moved-onto";

/// The commit a workspace last moved onto, upstream's: a catch-up and a
/// step that follows an update leave what is under it to upstream.
pub(crate) struct MovedOnto {
    path: std::path::PathBuf,
    last: Option<CommitId>,
}

impl MovedOnto {
    pub(crate) fn load(worker: &mut Worker) -> Result<Self, VcsError> {
        let path = worker
            .workspace()?
            .workspace_root()
            .join(".jj")
            .join(MOVED_ONTO);
        let last = std::fs::read_to_string(&path)
            .ok()
            .and_then(|hex| CommitId::try_from_hex(hex.trim()));
        Ok(Self { path, last })
    }

    pub(crate) fn last(&self) -> Option<&CommitId> {
        self.last.as_ref()
    }

    /// Records `onto`, when the run moved onto one.
    pub(crate) fn save(&self, onto: Option<&CommitId>) -> Result<(), VcsError> {
        if let Some(onto) = onto {
            std::fs::write(&self.path, onto.hex())?;
        }
        Ok(())
    }
}

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

/// Where `move_onto` goes: `onto`, unless an update moved `bookmark`
/// since the caller read `onto` (see [`upstream`]), to a commit that is
/// neither the run's own (an ancestor of `wc`'s parent) nor behind
/// `onto`. Moving onto `onto` would then point the bookmark back, or
/// aside, and the run goes onto the bookmark's commit instead.
fn past(
    tx: &Transaction,
    wc: &Commit,
    onto: &CommitId,
    bookmark: &str,
) -> Result<CommitId, VcsError> {
    let Some(target) = upstream(tx, bookmark) else {
        return Ok(onto.clone());
    };
    let index = tx.repo().index();
    let own = match wc.parent_ids() {
        [head] => block_on(index.is_ancestor(&target, head))?,
        _ => false,
    };
    if own || block_on(index.is_ancestor(&target, onto))? {
        return Ok(onto.clone());
    }
    Ok(target)
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
    // A run with no commit of its own, moved onto the root commit (a
    // repository whose trunk branch upstream deleted), has no newest
    // commit to name: the bookmark goes rather than name the root.
    let target = if head.id() == tx.repo().store().root_commit_id() {
        RefTarget::absent()
    } else {
        RefTarget::normal(head.id().clone())
    };
    tx.repo_mut()
        .set_local_bookmark_target(RefName::new(bookmark), target);
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
