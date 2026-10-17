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
//! # Today, and what replaces it
//!
//! Every head is a local jj bookmark: a run's is `tau/<run id>`, a
//! branch's is its name. jj-cli exports every local bookmark to
//! `refs/heads/` in a colocated repository, so in a repository of the
//! person's own a run's bookmark would become one of their branches
//! (ADR 0043). A run's head therefore moves out of bookmarks, to the Git
//! ref `refs/tau/runs/<run id>`, written with gix. Branches stay
//! bookmarks. Only this module changes:
//!
//! - **While the run's workspace exists, its head is that workspace's
//!   `@-`.** Every [`point`] names that commit today (a commit, a turn's
//!   end, a landing on the run, a catch-up), so jj already moves it with
//!   each rewrite, and [`point`] for a run only records which workspace
//!   is the run's. Its callers all have the workspace in hand, and pass
//!   it then.
//! - **`colocate::commit` rewrites the ref after each operation**, under
//!   the repository's lock, to that `@-`. A crash between the operation
//!   and the ref leaves the ref behind, which the rule above corrects
//!   while the workspace exists.
//! - **Once the workspace is gone** (a sub-agent whose work is kept,
//!   ADR 0042), the ref is the head. Other runs' rewrites can move its
//!   commit (a main chat's catch-up restacks the chats on its commits),
//!   so a read follows the commit's change id to its visible commit, as
//!   [`ProjectRepo::current`] does for links.
//! - **`vcs_undo` keeps working.** It merges the view back, which moves
//!   the workspace's `@` back, and with it `@-`: the head follows with
//!   no write of its own, and the ref is rewritten after the operation
//!   like any other. A bookmark only came back with the view because it
//!   is in it; the ref does not need to be.
//! - **A run's head has no upstream** ([`upstream`]): `refs/tau` is never
//!   fetched, as `tau/<run>` never was.
//! - **[`ProjectRepo::run_heads`] lists `refs/tau/runs/*`**, and
//!   [`ProjectRepo::remove_head`] deletes the ref with gix: no jj
//!   operation, so no `tau: remove bookmark` line in the operation log.
//! - The `tau/*` bookmarks of existing projects migrate in one
//!   operation, `tau: move run heads to refs/tau`.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::fmt;

use jj_lib::{
    backend::CommitId,
    op_store::RefTarget,
    ref_name::{RefName, RemoteName},
    repo::Repo,
    transaction::Transaction,
};
use tau_agent::tool::RunId;

use crate::{ProjectRepo, error::VcsError};

/// What the names of runs' heads start with, as
/// [`ProjectRepo::run_heads`] lists them.
pub const RUN_BOOKMARK_PREFIX: &str = "tau/";

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
        name.strip_prefix(RUN_BOOKMARK_PREFIX)
    }

    /// The bookmark the head is kept under.
    fn bookmark(&self) -> String {
        match self {
            Self::Run(run) => format!("{RUN_BOOKMARK_PREFIX}{}", run.0),
            Self::Branch(name) => name.clone(),
        }
    }
}

/// The name the head goes by: what [`ProjectRepo::run_heads`] lists, and
/// what the operation log says.
impl fmt::Display for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.bookmark())
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

/// The name of `run`'s head, as [`ProjectRepo::run_heads`] lists it.
pub fn run_head_name(run: &RunId) -> String {
    Head::Run(run.clone()).to_string()
}

/// The commit `head` names in `repo`, if it names one: none when it
/// has no commit, or conflicting ones.
pub(crate) fn target(repo: &dyn Repo, head: &Head) -> Option<CommitId> {
    repo.view()
        .get_local_bookmark(RefName::new(&head.bookmark()))
        .as_normal()
        .cloned()
}

/// Points `head` at `commit`. Returns whether it moved: false when it
/// named `commit` already.
pub(crate) fn point(
    tx: &mut Transaction,
    head: &Head,
    commit: &CommitId,
) -> bool {
    if target(tx.repo(), head).as_ref() == Some(commit) {
        return false;
    }
    tx.repo_mut().set_local_bookmark_target(
        RefName::new(&head.bookmark()),
        RefTarget::normal(commit.clone()),
    );
    true
}

/// Leaves `head` naming no commit: a run moved onto the root commit has
/// no newest commit of its own.
pub(crate) fn clear(tx: &mut Transaction, head: &Head) {
    tx.repo_mut().set_local_bookmark_target(
        RefName::new(&head.bookmark()),
        RefTarget::absent(),
    );
}

/// `head`'s commit when an update put it there: the source's branch of
/// that name, as `<head>@origin` names it. Only an update moves a head
/// to upstream's commit; a run's own head has no such branch, and a
/// branch a run moved names the run's own commit.
pub(crate) fn upstream(repo: &dyn Repo, head: &Head) -> Option<CommitId> {
    let bookmark = head.bookmark();
    let name = RefName::new(&bookmark);
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

impl ProjectRepo {
    /// The commit `head` names, as a full commit id in hex. `None` when
    /// it names none, or conflicting ones.
    pub fn head(&self, head: &Head) -> Result<Option<String>, VcsError> {
        self.bookmark(&head.bookmark())
    }

    /// The names of the runs' heads the project has, sorted: each run's
    /// is [`run_head_name`], which [`Head::run_of`] reads back.
    pub fn run_heads(&self) -> Result<Vec<String>, VcsError> {
        self.bookmarks(RUN_BOOKMARK_PREFIX)
    }

    /// Removes `head`, if the project has it. The commits it named stay.
    pub fn remove_head(&self, head: &Head) -> Result<(), VcsError> {
        self.remove_bookmark(&head.bookmark())
    }
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
