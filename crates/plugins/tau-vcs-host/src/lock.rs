//! One lock per repository around every operation tau-vcs writes
//! (`docs/reference/vcs.md`, "Threading"): runs work in workspaces of
//! one repository from threads of their own, and two operations that
//! start from the same one fork jj's operation log. jj merges the fork
//! on the next load, but a merge cannot always keep both sides' intent:
//! two rewrites of one commit leave it divergent, and two moves of one
//! bookmark leave it conflicted. Under the lock, operations happen one
//! after another, as if each started after the last one finished.
//!
//! The lock is a file in the repository's `.jj/repo`, locked with
//! `flock` through jj-lib's `FileLock`. Each holder opens the file on
//! its own, so it excludes other threads of this process and other
//! processes alike. It is not reentrant: code that holds it never takes
//! it again, and never waits on a job that takes it.
//!
//! The jj CLI never takes it. What the CLI does take is
//! `.jj/repo/git_import_export.lock` (jj-cli 0.46.0's
//! `lock_git_import_export`, in a colocated workspace), around its
//! snapshot and Git import, and again around writing its transaction
//! with the Git export and the `HEAD` and index reset that follow. tau
//! takes that one too, as [`GitLock`], around the same writes, so a
//! `jj` command and tau never write the Git store at once.
//!
//! The order is `tau.lock`, then `git_import_export.lock`, then a
//! working copy's own lock; [`RepoLock::git`] is the only way to the
//! second, so nothing takes it without the first. The CLI takes the
//! second, then a working copy's lock, and never the first: no order
//! of the two programs waits in a circle.

use std::path::{Path, PathBuf};

use jj_lib::lock::FileLock;

use crate::error::VcsError;

const FILE: &str = "tau.lock";
/// jj-cli's: `workspace.repo_path().join("git_import_export.lock")`.
pub(crate) const GIT_FILE: &str = "git_import_export.lock";

/// The repository's lock, held: see the module docs.
pub(crate) struct RepoLock {
    repo_path: PathBuf,
    _file: FileLock,
}

/// jj-cli's lock around Git import and export, held under a
/// [`RepoLock`]. Writing an operation takes one ([`crate::colocate`]).
pub(crate) struct GitLock {
    _file: FileLock,
}

/// Takes the lock of the repository whose store is at `repo_path` (a
/// workspace's `.jj/repo`), waiting while another holder has it.
pub(crate) fn lock_repo(repo_path: &Path) -> Result<RepoLock, VcsError> {
    Ok(RepoLock {
        repo_path: repo_path.to_owned(),
        _file: FileLock::lock(repo_path.join(FILE)).map_err(VcsError::Lock)?,
    })
}

impl RepoLock {
    /// Takes the repository's Git import/export lock as well, waiting
    /// while a `jj` command has it.
    pub(crate) fn git(&self) -> Result<GitLock, VcsError> {
        Ok(GitLock {
            _file: FileLock::lock(self.repo_path.join(GIT_FILE))
                .map_err(VcsError::Lock)?,
        })
    }
}
