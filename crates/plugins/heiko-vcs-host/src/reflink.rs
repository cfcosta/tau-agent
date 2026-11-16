//! A new run workspace's files as clones of heiko's other workspaces'
//! files, where the filesystem can share blocks (ADR 0061,
//! `docs/reference/vcs.md`, "Projects").
//!
//! [`check_out`] stands in for jj-lib's `Workspace::check_out` on a new
//! workspace: it first clones each file of the commit that a source
//! workspace holds unchanged ([`place`]), then lets jj's `check_out`
//! write the rest (it leaves a file that is there already, and marks it
//! for the next snapshot), then has jj's snapshot hash the clones, and
//! rewrites from the store whatever is not the commit's.
//!
//! A clone shares the source's blocks until either side writes:
//! `FICLONE` on Linux, `fclonefileat` on macOS. Bytes are never copied
//! and files never linked here; where a clone cannot be made, jj writes
//! the file.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    collections::{BTreeMap, HashSet},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use futures_util::StreamExt as _;
use jj_lib::{
    backend::{MillisSinceEpoch, TreeValue},
    commit::Commit,
    gitignore::GitIgnoreFile,
    local_working_copy::{FileType, TreeState, TreeStateSettings},
    matchers::{EverythingMatcher, NothingMatcher},
    merged_tree::MergedTree,
    op_store::OperationId,
    repo_path::RepoPathBuf,
    working_copy::SnapshotOptions,
    workspace::Workspace,
};
use pollster::block_on;

use crate::error::VcsError;

/// Makes `to`, which must not exist, a clone of the regular file
/// `from`.
pub(crate) type CloneFile = fn(&Path, &Path) -> io::Result<()>;

/// Checks `commit` out in the new `workspace`, whose working copy is
/// still empty, cloning what it can from the workspaces at `sources`,
/// and records the working copy at `operation`. Returns how many files
/// are clones.
pub(crate) fn check_out(
    workspace: &mut Workspace,
    operation: OperationId,
    commit: &Commit,
    sources: &[PathBuf],
) -> Result<usize, VcsError> {
    let settings =
        TreeStateSettings::try_from_user_settings(workspace.settings()).ok();
    let placed = match settings {
        Some(settings) => place(
            workspace.workspace_root(),
            &commit.tree(),
            sources,
            &settings,
            cloner(),
        ),
        None => 0,
    };
    let mut locked = block_on(workspace.start_working_copy_mutation())?;
    block_on(locked.locked_wc().check_out(commit))
        .map_err(VcsError::CheckOut)?;
    if placed > 0 {
        let options = SnapshotOptions {
            base_ignores: GitIgnoreFile::empty(),
            progress: None,
            start_tracking_matcher: &NothingMatcher,
            force_tracking_matcher: &NothingMatcher,
            max_new_file_size: u64::MAX,
        };
        let (found, _) = block_on(locked.locked_wc().snapshot(&options))?;
        if found.tree_ids() != commit.tree().tree_ids() {
            // A source changed between its stat and its clone: what
            // differs comes from the store.
            block_on(locked.locked_wc().check_out(commit))
                .map_err(VcsError::CheckOut)?;
        }
    }
    block_on(locked.finish(operation))?;
    Ok(placed)
}

/// Clones into the empty workspace at `root` each plain file of `tree`
/// that one of the workspaces at `sources` holds unchanged: its tree
/// has that file at that path, and the file on disk has the size and
/// modification time its last snapshot recorded. Returns how many it
/// cloned. Nothing when `clone` cannot clone on `root`'s filesystem.
///
/// Never fails: a file it cannot clone is left for jj to write.
pub(crate) fn place(
    root: &Path,
    tree: &MergedTree,
    sources: &[PathBuf],
    settings: &TreeStateSettings,
    clone: CloneFile,
) -> usize {
    if !can_clone(&root.join(".jj"), clone) {
        return 0;
    }
    let Some(device) = device(root) else {
        return 0;
    };
    let mut wanted = files(tree);
    let mut placed = 0;
    for source in sources {
        if wanted.is_empty() {
            break;
        }
        if device_of(source) != Some(device) {
            continue;
        }
        let Some(state) = recorded(root, source, tree, settings) else {
            continue;
        };
        let differ = differing(tree, state.current_tree());
        let states = state.file_states();
        let mut done = Vec::new();
        for (path, executable) in &wanted {
            if differ.contains(path) {
                continue;
            }
            let Some(recorded) = states.get(path) else {
                continue;
            };
            if !matches!(recorded.file_type, FileType::Normal { .. }) {
                continue;
            }
            let (Ok(from), Ok(to)) =
                (path.to_fs_path(source), path.to_fs_path(root))
            else {
                continue;
            };
            if !unchanged(&from, recorded.size, recorded.mtime) {
                continue;
            }
            if let Some(parent) = to.parent()
                && std::fs::create_dir_all(parent).is_err()
            {
                continue;
            }
            match clone(&from, &to) {
                Ok(()) => {}
                Err(error) if cannot_share(&error) => return placed,
                Err(_) => continue,
            }
            if set_mode(&to, *executable).is_err() {
                // jj writes it again, mode and all.
                let _ = std::fs::remove_file(&to);
                continue;
            }
            placed += 1;
            done.push(path.clone());
        }
        for path in done {
            wanted.remove(&path);
        }
    }
    placed
}

/// What the workspace at `source` recorded at its last snapshot, read
/// from a copy in the new workspace at `root`: jj's loading writes a
/// fresh record where it finds none, and a source is never written.
fn recorded(
    root: &Path,
    source: &Path,
    tree: &MergedTree,
    settings: &TreeStateSettings,
) -> Option<TreeState> {
    let copy = root.join(".jj").join("heiko-clone-source");
    let _ = std::fs::remove_dir_all(&copy);
    std::fs::create_dir(&copy).ok()?;
    let read = std::fs::copy(
        source.join(".jj").join("working_copy").join("tree_state"),
        copy.join("tree_state"),
    )
    .ok()
    .and_then(|_| {
        TreeState::load(
            tree.store().clone(),
            source.to_owned(),
            copy.clone(),
            settings,
        )
        .ok()
    });
    let _ = std::fs::remove_dir_all(&copy);
    read
}

/// The plain files of `tree`, each with whether it is executable:
/// resolved files only, none under a `.git` or `.jj` folder.
fn files(tree: &MergedTree) -> BTreeMap<RepoPathBuf, bool> {
    block_on(tree.entries().collect::<Vec<_>>())
        .into_iter()
        .filter_map(|(path, value)| {
            let value = value.ok()?.into_resolved().ok()??;
            let TreeValue::File { executable, .. } = value else {
                return None;
            };
            let reserved = path.components().any(|part| {
                let part = part.as_internal_str();
                part.eq_ignore_ascii_case(".git")
                    || part.eq_ignore_ascii_case(".jj")
            });
            (!reserved).then_some((path, executable))
        })
        .collect()
}

/// The paths where `tree` and `other` differ.
fn differing(tree: &MergedTree, other: &MergedTree) -> HashSet<RepoPathBuf> {
    block_on(
        tree.diff(other, &EverythingMatcher)
            .map(|entry| entry.path)
            .collect::<HashSet<_>>(),
    )
}

/// Whether the file at `path` is a regular file of `size` bytes last
/// modified at `mtime`, as jj records them.
fn unchanged(path: &Path, size: u64, mtime: MillisSinceEpoch) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    metadata.is_file()
        && metadata.len() == size
        && metadata.modified().ok().and_then(millis) == Some(mtime)
}

/// `time` as jj records a modification time.
fn millis(time: SystemTime) -> Option<MillisSinceEpoch> {
    let millis = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).ok()?,
        Err(before) => -i64::try_from(before.duration().as_millis()).ok()?,
    };
    Some(MillisSinceEpoch(millis))
}

/// The mode jj's checkout gives a file.
fn set_mode(path: &Path, executable: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, executable);
        Ok(())
    }
}

/// Whether `clone` can clone a file inside the folder `dir`: it clones
/// a probe there, and removes both.
fn can_clone(dir: &Path, clone: CloneFile) -> bool {
    let from = dir.join("heiko-clone-probe");
    let to = dir.join("heiko-clone-probe-clone");
    let _ = std::fs::remove_file(&from);
    let _ = std::fs::remove_file(&to);
    let cloned =
        std::fs::write(&from, b"probe").is_ok() && clone(&from, &to).is_ok();
    let _ = std::fs::remove_file(&from);
    let _ = std::fs::remove_file(&to);
    cloned
}

/// Whether `error` says the filesystems cannot share blocks, so no
/// further clone will work either.
fn cannot_share(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(unix)]
    {
        use rustix::io::Errno;
        let cannot = [
            Errno::OPNOTSUPP,
            #[cfg(target_vendor = "apple")]
            Errno::NOTSUP,
            Errno::XDEV,
            Errno::INVAL,
            Errno::NOTTY,
            Errno::NOSYS,
        ];
        Errno::from_io_error(error).is_some_and(|errno| cannot.contains(&errno))
    }
    #[cfg(not(unix))]
    false
}

/// The device the folder at `path` is on.
fn device(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(path).ok().map(|metadata| metadata.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// [`device`], for a source workspace that heiko made whole.
fn device_of(source: &Path) -> Option<u64> {
    crate::workspace_init::is_made(source)
        .then(|| device(source))
        .flatten()
}

/// The clone [`check_out`] makes: [`clone_file`], or in a test the one
/// [`with_clone`] sets.
fn cloner() -> CloneFile {
    #[cfg(test)]
    if let Some(clone) = TEST_CLONE.get() {
        return clone;
    }
    clone_file
}

#[cfg(test)]
thread_local! {
    static TEST_CLONE: std::cell::Cell<Option<CloneFile>> =
        const { std::cell::Cell::new(None) };
}

/// Runs `body` with `clone` as the clone this thread's
/// [`check_out`] makes.
#[cfg(test)]
pub(crate) fn with_clone<T>(clone: CloneFile, body: impl FnOnce() -> T) -> T {
    TEST_CLONE.set(Some(clone));
    let out = body();
    TEST_CLONE.set(None);
    out
}

/// Makes `to`, which must not exist, share the blocks of the regular
/// file `from`, never following a symlink at either.
#[cfg(target_os = "linux")]
pub(crate) fn clone_file(from: &Path, to: &Path) -> io::Result<()> {
    let source = open_regular(from)?;
    let target = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    rustix::fs::ioctl_ficlone(&target, &source).map_err(|error| {
        let _ = std::fs::remove_file(to);
        io::Error::from(error)
    })
}

/// Makes `to`, which must not exist, share the blocks of the regular
/// file `from`, never following a symlink at either.
#[cfg(target_vendor = "apple")]
pub(crate) fn clone_file(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, CloneFlags, fclonefileat};
    let source = open_regular(from)?;
    Ok(fclonefileat(&source, CWD, to, CloneFlags::NOFOLLOW)?)
}

/// No clone where the platform has none: jj writes every file.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub(crate) fn clone_file(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// `path` opened to read, when it is a regular file and not a symlink;
/// a FIFO put in its place does not block the open.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn open_regular(path: &Path) -> io::Result<rustix::fd::OwnedFd> {
    use rustix::fs::{FileType as Kind, Mode, OFlags, fstat, open};
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if Kind::from_raw_mode(fstat(&fd)?.st_mode) != Kind::RegularFile {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, os::unix::fs::PermissionsExt as _};

    use heiko_testing::git::git;

    use super::*;
    use crate::{project::ProjectRepo, vcs::Identity};

    thread_local! {
        static CLONED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    /// A clone that copies the bytes and records the clone, standing in
    /// for a filesystem that shares blocks.
    fn copy(from: &Path, to: &Path) -> io::Result<()> {
        let bytes = std::fs::read(from)?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .and_then(|mut file| io::Write::write_all(&mut file, &bytes))?;
        CLONED.with_borrow_mut(|cloned| cloned.push(to.to_owned()));
        Ok(())
    }

    /// A clone on a filesystem that cannot share blocks, as ext4's.
    fn unsupported(_: &Path, _: &Path) -> io::Result<()> {
        Err(io::Error::from(rustix::io::Errno::OPNOTSUPP))
    }

    /// The files [`copy`] cloned into `dir` since the last call, by
    /// path from `dir`, without the probe.
    fn cloned_into(dir: &Path) -> Vec<String> {
        let mut cloned: Vec<String> = CLONED
            .take()
            .iter()
            .filter_map(|to| to.strip_prefix(dir).ok())
            .map(|path| path.display().to_string())
            .filter(|path| !path.starts_with(".jj"))
            .collect();
        cloned.sort();
        cloned
    }

    /// A project imported from a repository whose one commit holds
    /// `a.txt` (`one`), `dir/run.sh` (executable) and a symlink `link`
    /// to `a.txt`; and its trunk.
    fn project(home: &Path) -> (ProjectRepo, String) {
        let src = home.join("src");
        std::fs::create_dir_all(src.join("dir")).unwrap();
        std::fs::write(src.join("a.txt"), "one\n").unwrap();
        let script = src.join("dir/run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(
            &script,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("a.txt", src.join("link")).unwrap();
        git(&src, &["init", "--quiet"]);
        git(&src, &["add", "."]);
        git(&src, &["commit", "--quiet", "-m", "first"]);
        let project = ProjectRepo::import(
            src.to_str().unwrap(),
            home.join("p"),
            Identity::default(),
        )
        .unwrap();
        let trunk = project.trunk().unwrap();
        (project, trunk)
    }

    /// Whether the workspace `vcs` is in has no change on its commit's
    /// parent: what heiko's next snapshot finds there.
    fn clean(vcs: &crate::vcs::Vcs) -> bool {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(vcs.working_copy_diff())
            .unwrap()
            .starts_with("No changes")
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// A new run takes clones of the plain files another run's
    /// workspace holds unchanged, with the commit's modes; jj writes the
    /// symlink, and the workspace is clean.
    #[test]
    fn a_run_clones_the_files_another_workspace_holds() {
        let home = tempfile::tempdir().unwrap();
        let (project, trunk) = project(home.path());
        project.add_workspace("source", &trunk).unwrap();
        let vcs =
            with_clone(copy, || project.add_workspace("r", &trunk)).unwrap();
        let dir = project.workspace_dir("r");
        assert_eq!(cloned_into(&dir), ["a.txt", "dir/run.sh"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(mode(&dir.join("a.txt")), 0o644);
        assert_eq!(mode(&dir.join("dir/run.sh")), 0o755);
        assert_eq!(
            std::fs::read_link(dir.join("link")).unwrap(),
            Path::new("a.txt")
        );
        assert!(clean(&vcs));
    }

    /// A source file that changed since its workspace's last snapshot is
    /// not cloned: jj writes the commit's.
    #[test]
    fn a_changed_source_file_is_not_cloned() {
        let home = tempfile::tempdir().unwrap();
        let (project, trunk) = project(home.path());
        project.add_workspace("source", &trunk).unwrap();
        let source = project.workspace_dir("source").join("a.txt");
        std::fs::write(&source, "changed\n").unwrap();
        let vcs =
            with_clone(copy, || project.add_workspace("r", &trunk)).unwrap();
        let dir = project.workspace_dir("r");
        assert_eq!(cloned_into(&dir), ["dir/run.sh"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(clean(&vcs));
    }

    /// A source file changed behind its record (same size, its time set
    /// back) is cloned, and the snapshot that checks the clones puts the
    /// commit's bytes back.
    #[test]
    fn a_clone_of_a_file_changed_behind_its_record_is_rewritten() {
        let home = tempfile::tempdir().unwrap();
        let (project, trunk) = project(home.path());
        project.add_workspace("source", &trunk).unwrap();
        let source = project.workspace_dir("source").join("a.txt");
        let then = std::fs::metadata(&source).unwrap().modified().unwrap();
        std::fs::write(&source, "two\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(then)
            .unwrap();
        let vcs =
            with_clone(copy, || project.add_workspace("r", &trunk)).unwrap();
        let dir = project.workspace_dir("r");
        assert_eq!(cloned_into(&dir), ["a.txt", "dir/run.sh"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "two\n");
        assert!(clean(&vcs));
    }

    /// Where the filesystem cannot share blocks nothing is cloned, and
    /// jj writes every file.
    #[test]
    fn where_nothing_can_be_cloned_jj_writes_every_file() {
        let home = tempfile::tempdir().unwrap();
        let (project, trunk) = project(home.path());
        project.add_workspace("source", &trunk).unwrap();
        let vcs =
            with_clone(unsupported, || project.add_workspace("r", &trunk))
                .unwrap();
        let dir = project.workspace_dir("r");
        assert!(cloned_into(&dir).is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(mode(&dir.join("dir/run.sh")), 0o755);
        assert!(clean(&vcs));
    }

    /// A run clones from whichever source holds a file unchanged: the
    /// file one source changed comes from another.
    #[test]
    fn a_run_clones_each_file_from_a_source_that_holds_it() {
        let home = tempfile::tempdir().unwrap();
        let (project, trunk) = project(home.path());
        project.add_workspace("first", &trunk).unwrap();
        project.add_workspace("second", &trunk).unwrap();
        let first = project.workspace_dir("first");
        std::fs::write(first.join("a.txt"), "changed\n").unwrap();
        let vcs =
            with_clone(copy, || project.add_workspace("r", &trunk)).unwrap();
        let dir = project.workspace_dir("r");
        assert_eq!(cloned_into(&dir), ["a.txt", "dir/run.sh"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(clean(&vcs));
    }

    /// Reading a source's record writes nothing in the source.
    #[test]
    fn a_source_is_never_written() {
        let home = tempfile::tempdir().unwrap();
        let (project, trunk) = project(home.path());
        project.add_workspace("source", &trunk).unwrap();
        let state = project
            .workspace_dir("source")
            .join(".jj/working_copy/tree_state");
        let before = std::fs::read(&state).unwrap();
        let modified = std::fs::metadata(&state).unwrap().modified().unwrap();
        with_clone(copy, || project.add_workspace("r", &trunk)).unwrap();
        CLONED.take();
        assert_eq!(std::fs::read(&state).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&state).unwrap().modified().unwrap(),
            modified
        );
    }

    /// Neither a symlink nor a FIFO in a source's place is cloned, and
    /// the FIFO does not block; a clone never replaces a file.
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn a_clone_takes_only_a_regular_file_into_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        std::fs::write(dir.join("file"), "bytes").unwrap();
        std::os::unix::fs::symlink("file", dir.join("link")).unwrap();
        assert!(clone_file(&dir.join("link"), &dir.join("from-link")).is_err());
        assert!(!dir.join("from-link").exists());
        rustix::fs::mknodat(
            rustix::fs::CWD,
            dir.join("fifo"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        assert!(clone_file(&dir.join("fifo"), &dir.join("from-fifo")).is_err());
        assert!(!dir.join("from-fifo").exists());
        std::fs::write(dir.join("kept"), "kept").unwrap();
        assert!(clone_file(&dir.join("file"), &dir.join("kept")).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("kept")).unwrap(), "kept");
    }

    /// The directory `HEIKO_TEST_REFLINK_DIR` names, on a filesystem that
    /// shares blocks (Btrfs, XFS with reflinks, APFS); none when unset.
    fn reflink_dir() -> Option<tempfile::TempDir> {
        let Some(dir) = std::env::var_os("HEIKO_TEST_REFLINK_DIR") else {
            eprintln!(
                "HEIKO_TEST_REFLINK_DIR is unset: the real clone is not tried"
            );
            return None;
        };
        Some(tempfile::tempdir_in(dir).unwrap())
    }

    /// On a filesystem that shares blocks, a clone has the source's bytes
    /// and a write to either leaves the other as it was.
    #[test]
    fn a_real_clone_is_independent_of_its_source() {
        let Some(dir) = reflink_dir() else { return };
        let dir = dir.path();
        std::fs::write(dir.join("from"), "source bytes").unwrap();
        clone_file(&dir.join("from"), &dir.join("to")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("to")).unwrap(),
            "source bytes"
        );
        std::fs::write(dir.join("to"), "clone bytes").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("from")).unwrap(),
            "source bytes"
        );
    }

    /// On a filesystem that shares blocks, a new run's plain files are
    /// real clones and the workspace is clean.
    #[test]
    fn a_run_on_a_real_reflink_filesystem_clones_its_files() {
        fn recorded(from: &Path, to: &Path) -> io::Result<()> {
            clone_file(from, to)?;
            CLONED.with_borrow_mut(|cloned| cloned.push(to.to_owned()));
            Ok(())
        }
        let Some(home) = reflink_dir() else { return };
        let (project, trunk) = project(home.path());
        project.add_workspace("source", &trunk).unwrap();
        let vcs = with_clone(recorded, || project.add_workspace("r", &trunk))
            .unwrap();
        let dir = project.workspace_dir("r");
        assert_eq!(cloned_into(&dir), ["a.txt", "dir/run.sh"]);
        assert!(clean(&vcs));
    }
}
