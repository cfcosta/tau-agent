//! Where a run's newest commit is kept: its [`Head`]
//! (`docs/reference/vcs.md`, "Run heads").
//!
//! A run's commits move a head as they are made, landed, caught up and
//! undone, and the host reads it back to land the run, open a pull
//! request from it, keep or drop a sub-agent's work, and sweep what a
//! closed run left. A run's own head is [`Head::Run`]; a main chat,
//! which commits on trunk, moves the branch [`Head::Branch`]. Every
//! reader and writer goes through this module, so nothing else knows
//! how a head is stored.
//!
//! A branch's head is its local jj bookmark. A run's is not a bookmark
//! (ADR 0043): jj-cli exports every local bookmark to `refs/heads/` in a
//! colocated repository, so in a repository of the person's own a run's
//! bookmark would become one of their branches. It is this:
//!
//! - **While the run's workspace exists, its head is that workspace's
//!   `@-`.** Every [`point`] names that commit (a commit, a turn's end,
//!   a landing on the run, a catch-up), so jj already moves it with each
//!   rewrite, and [`point`] for a run only records which workspace is
//!   the run's: a file in the workspace's `.jj`, [`CLAIM`], naming the
//!   run, which goes with the workspace's directory.
//! - **tau writes the Git ref `refs/tau/runs/<run id>` with gix after
//!   each operation** ([`settle`], from `colocate::commit`, under the
//!   repository's locks), at that `@-`. `git branch` and `git push --all`
//!   never see it, and jj's import leaves `refs/tau/` alone, so it never
//!   becomes a bookmark. A crash between the operation and the ref
//!   leaves the ref behind, which the rule above corrects while the
//!   workspace exists, and the next operation writes again.
//! - **Once the workspace is gone** (a sub-agent whose work is kept,
//!   ADR 0042), the ref is the head: `forget_workspace` writes it first.
//!   Other runs' rewrites can move its commit (a main chat's catch-up
//!   restacks the chats on its commits), so a read follows the commit's
//!   change id to its visible commit, as [`ProjectRepo::current`] does
//!   for links, and [`settle`] moves the ref there. A ref whose commit
//!   is hidden with no visible successor, abandoned, or missing names
//!   no head: the run's work was dropped, and nothing brings it back.
//! - **`vcs_undo` keeps working.** It merges the view back, which moves
//!   the workspace's `@` back, and with it `@-`: the head follows with
//!   no write of its own, and the ref is rewritten after the operation
//!   like any other.
//! - **A run's head has no upstream** ([`upstream`]): `refs/tau` is never
//!   fetched.
//!
//! Projects from before kept runs' heads as `tau/<run id>` bookmarks;
//! [`ProjectRepo::move_run_heads`] moves them to their refs as the
//! project opens.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{collections::BTreeMap, fmt, path::PathBuf};

use gix::refs::{
    FullName,
    Target,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
};
use jj_lib::{
    backend::CommitId,
    git::{REMOTE_NAME_FOR_LOCAL_GIT_REPO, get_git_backend},
    object_id::ObjectId as _,
    op_store::RefTarget,
    ref_name::{RefName, RefNameBuf, RemoteName, WorkspaceName},
    repo::{ReadonlyRepo, Repo},
    transaction::Transaction,
};
use pollster::block_on;
use tau_agent::tool::RunId;

use crate::{ProjectRepo, error::VcsError};

/// Where runs' heads live in the Git store, and what the names
/// [`ProjectRepo::run_heads`] lists start with.
pub const RUN_HEAD_PREFIX: &str = "refs/tau/runs/";

/// The file in a workspace's `.jj` that names the run whose head the
/// workspace's `@-` is.
const CLAIM: &str = "tau-run";

/// What runs' heads were named when they were bookmarks, before
/// [`ProjectRepo::move_run_heads`].
const OLD_PREFIX: &str = "tau/";

/// The head a run's commits move.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Head {
    /// A run's own: its newest commit.
    Run(RunId),
    /// A branch, by name: trunk's, which a main chat commits on.
    Branch(String),
}

impl Head {
    /// The run whose head goes by `name`, as [`ProjectRepo::run_heads`]
    /// lists it; `None` for a name that is not a run's head.
    pub fn run_of(name: &str) -> Option<&str> {
        name.strip_prefix(RUN_HEAD_PREFIX)
    }
}

/// The name the head goes by: what [`ProjectRepo::run_heads`] lists, and
/// what the operation log says.
impl fmt::Display for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Run(run) => write!(f, "{RUN_HEAD_PREFIX}{}", run.0),
            Self::Branch(name) => f.write_str(name),
        }
    }
}

impl From<&str> for Head {
    fn from(name: &str) -> Self {
        Self::Branch(name.to_owned())
    }
}

impl From<String> for Head {
    fn from(name: String) -> Self {
        Self::Branch(name)
    }
}

impl From<&String> for Head {
    fn from(name: &String) -> Self {
        Self::Branch(name.clone())
    }
}

impl From<&Head> for Head {
    fn from(head: &Head) -> Self {
        head.clone()
    }
}

/// The name of `run`'s head, as [`ProjectRepo::run_heads`] lists it:
/// its ref's.
pub fn run_head_name(run: &RunId) -> String {
    Head::Run(run.clone()).to_string()
}

/// `run`'s ref, refused when its id cannot be part of one.
fn ref_name(run: &str) -> Result<FullName, VcsError> {
    FullName::try_from(format!("{RUN_HEAD_PREFIX}{run}"))
        .map_err(|_| VcsError::RunHeadName(run.to_owned()))
}

/// The commit branch `name`'s bookmark names in `repo`, if it names
/// one: none when it has no commit, or conflicting ones.
fn bookmark_target(repo: &dyn Repo, name: &str) -> Option<CommitId> {
    repo.view()
        .get_local_bookmark(RefName::new(name))
        .as_normal()
        .cloned()
}

/// Points `head` at `commit`, the newest commit of the run working in
/// `workspace`: its working copy's parent. Returns whether the view
/// moved: false when the branch named `commit` already, and for a run's
/// head, which is that parent already and only records that `workspace`
/// is the run's.
pub(crate) fn point(
    tx: &mut Transaction,
    head: &Head,
    workspace: &WorkspaceName,
    commit: &CommitId,
) -> Result<bool, VcsError> {
    match head {
        Head::Run(run) => {
            claim(tx.base_repo(), workspace, &run.0)?;
            Ok(false)
        }
        Head::Branch(name) => {
            if bookmark_target(tx.repo(), name).as_ref() == Some(commit) {
                return Ok(false);
            }
            tx.repo_mut().set_local_bookmark_target(
                RefName::new(name),
                RefTarget::normal(commit.clone()),
            );
            Ok(true)
        }
    }
}

/// Leaves `head` naming no commit: a run in `workspace` moved onto the
/// root commit has no newest commit of its own. A run's head names none
/// while its workspace's `@-` is the root commit.
pub(crate) fn clear(
    tx: &mut Transaction,
    head: &Head,
    workspace: &WorkspaceName,
) -> Result<(), VcsError> {
    match head {
        Head::Run(run) => claim(tx.base_repo(), workspace, &run.0),
        Head::Branch(name) => {
            tx.repo_mut().set_local_bookmark_target(
                RefName::new(name),
                RefTarget::absent(),
            );
            Ok(())
        }
    }
}

/// `head`'s commit when an update put it there: the source's branch of
/// that name, as `<head>@origin` names it. Only an update moves a head
/// to upstream's commit; a run's own head has no such branch, and a
/// branch a run moved names the run's own commit.
pub(crate) fn upstream(repo: &dyn Repo, head: &Head) -> Option<CommitId> {
    let Head::Branch(branch) = head else {
        return None;
    };
    let name = RefName::new(branch);
    let view = repo.view();
    let local = view.get_local_bookmark(name).as_normal()?;
    let remote = view
        .get_remote_bookmark(
            name.to_remote_symbol(RemoteName::new(crate::project::REMOTE)),
        )
        .target
        .as_normal()?;
    (local == remote).then(|| local.clone())
}

/// Where workspace `name`'s files are, as the workspace store has it.
fn workspace_root(
    repo: &ReadonlyRepo,
    name: &WorkspaceName,
) -> Result<Option<PathBuf>, VcsError> {
    Ok(repo.loader().workspace_store().get_workspace_path(name)?)
}

/// The run whose head workspace `name`'s `@-` is, if one is.
fn claimant(
    repo: &ReadonlyRepo,
    name: &WorkspaceName,
) -> Result<Option<String>, VcsError> {
    let Some(root) = workspace_root(repo, name)? else {
        return Ok(None);
    };
    match std::fs::read_to_string(root.join(".jj").join(CLAIM)) {
        Ok(run) => Ok(Some(run)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Records that `run`'s head is workspace `name`'s `@-`, and no other
/// workspace's. Writes only what changed: once the run's first turn
/// ended, nothing.
fn claim(
    repo: &ReadonlyRepo,
    name: &WorkspaceName,
    run: &str,
) -> Result<(), VcsError> {
    ref_name(run)?;
    if claimant(repo, name)?.as_deref() == Some(run) {
        return Ok(());
    }
    let Some(root) = workspace_root(repo, name)? else {
        return Err(VcsError::NoWorkingCopy);
    };
    for other in repo.view().wc_commit_ids().keys() {
        if &**other != name
            && claimant(repo, other)?.as_deref() == Some(run)
            && let Some(other) = workspace_root(repo, other)?
        {
            std::fs::remove_file(other.join(".jj").join(CLAIM))?;
        }
    }
    let path = root.join(".jj").join(CLAIM);
    let written = path.with_extension("new");
    std::fs::write(&written, run)?;
    std::fs::rename(&written, &path)?;
    Ok(())
}

/// The newest commit of the run whose working copy is `wc`: its parent,
/// unless that is the root commit.
fn newest(
    repo: &dyn Repo,
    wc: &CommitId,
) -> Result<Option<CommitId>, VcsError> {
    let wc = repo.store().get_commit(wc)?;
    Ok(wc
        .parent_ids()
        .first()
        .filter(|parent| *parent != repo.store().root_commit_id())
        .cloned())
}

/// Where `commit`'s change is now in `repo`: `commit` while it is
/// visible, else the one visible commit its change names. `None` when
/// nothing of it is visible (abandoned, or hidden with no successor),
/// when its change is divergent, or when `repo` never had it.
fn follow(
    repo: &dyn Repo,
    commit: &CommitId,
) -> Result<Option<CommitId>, VcsError> {
    if !block_on(repo.index().has_id(commit))? {
        return Ok(None);
    }
    let Ok(commit) = repo.store().get_commit(commit) else {
        return Ok(None);
    };
    let Some(targets) = block_on(repo.resolve_change_id(commit.change_id()))?
    else {
        return Ok(None);
    };
    let visible: Vec<&CommitId> =
        targets.visible_with_offsets().map(|(_, id)| id).collect();
    if visible.contains(&commit.id()) {
        return Ok(Some(commit.id().clone()));
    }
    Ok(match visible.as_slice() {
        [id] => Some((*id).clone()),
        _ => None,
    })
}

/// The Git store of `repo`, if it has one.
fn git_store(repo: &ReadonlyRepo) -> Result<Option<gix::Repository>, VcsError> {
    let Ok(backend) = get_git_backend(repo.store()) else {
        return Ok(None);
    };
    crate::clone::open(backend.git_repo_path())
        .map(Some)
        .map_err(|error| VcsError::RunHeads(error.to_string()))
}

/// The runs' refs in `git`, by run id, at their commits.
fn run_refs(
    git: &gix::Repository,
) -> Result<BTreeMap<String, CommitId>, VcsError> {
    let error =
        |error: &dyn fmt::Display| VcsError::RunHeads(error.to_string());
    let mut refs = BTreeMap::new();
    let references = git.references().map_err(|e| error(&e))?;
    for reference in references
        .prefixed(RUN_HEAD_PREFIX)
        .map_err(|e| error(&e))?
    {
        let reference = reference.map_err(|e| error(&e))?;
        let Some(id) = reference.target().try_id().map(ToOwned::to_owned)
        else {
            continue;
        };
        let name = reference.name().as_bstr().to_string();
        if let Some(run) = Head::run_of(&name) {
            refs.insert(run.to_owned(), CommitId::from_bytes(id.as_bytes()));
        }
    }
    Ok(refs)
}

/// The edit that leaves `run`'s ref at `commit`, or deletes it.
fn ref_edit(run: &str, commit: Option<&CommitId>) -> Result<RefEdit, VcsError> {
    let change = match commit {
        Some(commit) => Change::Update {
            log: LogChange::default(),
            expected: PreviousValue::Any,
            new: Target::Object(gix::ObjectId::from_bytes_or_panic(
                commit.as_bytes(),
            )),
        },
        None => Change::Delete {
            expected: PreviousValue::Any,
            log: RefLog::AndReference,
        },
    };
    Ok(RefEdit {
        change,
        name: ref_name(run)?,
        deref: false,
    })
}

fn edit_refs(
    git: &gix::Repository,
    edits: Vec<RefEdit>,
) -> Result<(), VcsError> {
    if edits.is_empty() {
        return Ok(());
    }
    git.edit_references(edits)
        .map_err(|error| VcsError::RunHeads(error.to_string()))?;
    Ok(())
}

/// Each claimed run's head in `repo`, by run id: the `@-` of the
/// workspace that claims it.
fn claimed_heads(
    repo: &ReadonlyRepo,
) -> Result<BTreeMap<String, Option<CommitId>>, VcsError> {
    let mut heads = BTreeMap::new();
    for (name, wc) in repo.view().wc_commit_ids() {
        if let Some(run) = claimant(repo, name)? {
            heads.insert(run, newest(repo, wc)?);
        }
    }
    Ok(heads)
}

/// Writes the runs' refs for `repo`, as an operation just left it:
/// each claimed run's at its workspace's `@-` ([`claimed_heads`]), and
/// each other run's followed to where its commit's change is now, or
/// left as it is when nothing of it is visible. Only refs that change
/// are written. The caller holds the repository's locks.
pub(crate) fn settle(repo: &ReadonlyRepo) -> Result<(), VcsError> {
    let Some(git) = git_store(repo)? else {
        return Ok(());
    };
    let refs = run_refs(&git)?;
    let claimed = claimed_heads(repo)?;
    let mut edits = Vec::new();
    for (run, head) in &claimed {
        if refs.get(run) != head.as_ref() {
            edits.push(ref_edit(run, head.as_ref())?);
        }
    }
    for (run, commit) in &refs {
        if claimed.contains_key(run) {
            continue;
        }
        if let Some(now) = follow(repo, commit)?
            && &now != commit
        {
            edits.push(ref_edit(run, Some(&now))?);
        }
    }
    edit_refs(&git, edits)
}

/// `run`'s head in `repo`: its workspace's `@-` while it has one, else
/// its ref, followed to where its change is now.
fn run_head(
    repo: &ReadonlyRepo,
    run: &str,
) -> Result<Option<CommitId>, VcsError> {
    for (name, wc) in repo.view().wc_commit_ids() {
        if claimant(repo, name)?.as_deref() == Some(run) {
            return newest(repo, wc);
        }
    }
    let Some(git) = git_store(repo)? else {
        return Ok(None);
    };
    match run_refs(&git)?.get(run) {
        Some(commit) => follow(repo, commit),
        None => Ok(None),
    }
}

impl ProjectRepo {
    /// The commit `head` names, as a full commit id in hex. `None` when
    /// it names none, or conflicting ones, and for a run whose work was
    /// dropped: its head's commit hidden with no successor, abandoned,
    /// or gone.
    pub fn head(&self, head: &Head) -> Result<Option<String>, VcsError> {
        match head {
            Head::Run(run) => {
                let repo = self.load()?;
                Ok(run_head(&repo, &run.0)?.map(|id| id.hex()))
            }
            Head::Branch(name) => self.bookmark(name),
        }
    }

    /// The names of the runs' heads the project has, sorted: each run's
    /// is [`run_head_name`], which [`Head::run_of`] reads back. A run has
    /// one once its ref is written, or while a workspace of its has
    /// claimed it.
    pub fn run_heads(&self) -> Result<Vec<String>, VcsError> {
        let repo = self.load()?;
        let mut runs: Vec<String> = match git_store(&repo)? {
            Some(git) => run_refs(&git)?.into_keys().collect(),
            None => Vec::new(),
        };
        for name in repo.view().wc_commit_ids().keys() {
            runs.extend(claimant(&repo, name)?);
        }
        let mut names: Vec<String> = runs
            .into_iter()
            .map(|run| format!("{RUN_HEAD_PREFIX}{run}"))
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Removes `head`, if the project has it. The commits it named stay.
    /// A run's goes from Git and from the workspace that claimed it,
    /// with no jj operation.
    pub fn remove_head(&self, head: &Head) -> Result<(), VcsError> {
        let Head::Run(run) = head else {
            return self.remove_bookmark(&head.to_string());
        };
        let lock = self.lock()?;
        let _git = lock.git()?;
        let repo = self.load()?;
        for name in repo.view().wc_commit_ids().keys() {
            if claimant(&repo, name)?.as_deref() == Some(&*run.0)
                && let Some(root) = workspace_root(&repo, name)?
            {
                std::fs::remove_file(root.join(".jj").join(CLAIM))?;
            }
        }
        if let Some(git) = git_store(&repo)?
            && run_refs(&git)?.contains_key(&*run.0)
        {
            edit_refs(&git, vec![ref_edit(&run.0, None)?])?;
        }
        Ok(())
    }

    /// Writes the runs' refs for the repository as it is now, before an
    /// operation that forgets a workspace: once it is gone, the ref is
    /// its run's head.
    pub(crate) fn settle_heads(&self) -> Result<(), VcsError> {
        let repo = self.load()?;
        settle(&repo)
    }

    /// Moves the runs' heads of a project made before they were refs:
    /// each local bookmark `tau/<run id>` with no remote of that name (a
    /// pull request's branch has one) becomes `refs/tau/runs/<run id>`,
    /// at its commit, and the bookmark goes, with the Git branch jj
    /// exported for it, in one operation, `tau: move run heads to
    /// refs/tau`. A bookmark with conflicting commits names no head, and
    /// goes with no ref. Done again from the start when cut off: the
    /// refs are written before the operation, from the bookmarks it has
    /// not removed yet. Returns whether it moved any.
    pub(crate) fn move_run_heads(&self) -> Result<bool, VcsError> {
        if old_run_bookmarks(&*self.load()?).is_empty() {
            return Ok(false);
        }
        let lock = self.lock()?;
        let git_lock = lock.git()?;
        let repo = self.load()?;
        let old = old_run_bookmarks(&*repo);
        if old.is_empty() {
            return Ok(false);
        }
        if let Some(git) = git_store(&repo)? {
            let edits = old
                .iter()
                .filter_map(|(name, target)| {
                    let run = name.as_str().strip_prefix(OLD_PREFIX)?;
                    Some(ref_edit(run, Some(target.as_ref()?)))
                })
                .collect::<Result<Vec<_>, _>>()?;
            edit_refs(&git, edits)?;
        }
        let mut tx = repo.start_transaction();
        for (name, _) in &old {
            tx.repo_mut()
                .set_local_bookmark_target(name, RefTarget::absent());
        }
        crate::colocate::commit(tx, "move run heads to refs/tau", &git_lock)?;
        Ok(true)
    }
}

/// The local bookmarks that are runs' heads from before they were refs
/// ([`ProjectRepo::move_run_heads`]), with their commits: `tau/<run id>`,
/// with no remote bookmark of that name, and a run id that can name a
/// ref.
fn old_run_bookmarks(repo: &dyn Repo) -> Vec<(RefNameBuf, Option<CommitId>)> {
    let view = repo.view();
    // Not jj's record of what it exported to the Git store, which it
    // keeps as the remote `git`.
    let remote: std::collections::HashSet<&RefName> = view
        .all_remote_bookmarks()
        .filter(|(symbol, _)| symbol.remote != REMOTE_NAME_FOR_LOCAL_GIT_REPO)
        .map(|(symbol, _)| symbol.name)
        .collect();
    view.local_bookmarks()
        .filter(|(name, _)| {
            name.as_str()
                .strip_prefix(OLD_PREFIX)
                .is_some_and(|run| ref_name(run).is_ok())
                && !remote.contains(name)
        })
        .map(|(name, target)| (name.to_owned(), target.as_normal().cloned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    /// A run's head's name reads back as that run, and no two runs'
    /// heads share a name: the sweep takes a head by its name.
    #[hegel::test(test_cases = 300)]
    fn a_run_heads_name_reads_back_as_its_run(tc: TestCase) {
        let id = || gs::text().max_size(6).alphabet("ab/-é");
        let one: String = tc.draw(id());
        let other: String = tc.draw(id());
        let name = run_head_name(&RunId(one.as_str().into()));
        assert_eq!(Head::run_of(&name), Some(one.as_str()));
        let other_name = run_head_name(&RunId(other.as_str().into()));
        assert_eq!(name == other_name, one == other);
    }
}
