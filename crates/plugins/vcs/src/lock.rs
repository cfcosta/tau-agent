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

use std::path::Path;

use jj_lib::lock::FileLock;

use crate::error::VcsError;

const FILE: &str = "tau.lock";

/// Takes the lock of the repository whose store is at `repo_path` (a
/// workspace's `.jj/repo`), waiting while another holder has it.
pub(crate) fn lock_repo(repo_path: &Path) -> Result<FileLock, VcsError> {
    FileLock::lock(repo_path.join(FILE)).map_err(VcsError::Lock)
}
