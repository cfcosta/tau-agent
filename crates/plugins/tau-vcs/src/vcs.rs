//! [`Vcs`]: a handle on one jj workspace (`docs/reference/vcs.md`,
//! "Threading"; ADR 0027).
//!
//! jj-lib's futures are not `Send`, and much of their work is blocking
//! file and object I/O. So the workspace sits behind an async lock, and
//! each job takes the lock, then runs on tokio's `spawn_blocking`,
//! driving jj-lib's futures with `pollster`: jobs run one at a time, in
//! the order they asked, and no thread waits between them. A job that
//! panics is caught there: its caller gets an error, and the workspace
//! is loaded again for the next job, since jj-lib's state after a panic
//! cannot be trusted.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0027)"
)]

use std::{
    any::Any,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::Arc,
};

use jj_lib::{
    config::{ConfigLayer, ConfigSource, StackedConfig},
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
    },
    settings::UserSettings,
    workspace::Workspace,
};
use pollster::block_on;
use tokio::sync::Mutex;

use crate::error::VcsError;

/// Who the commits and operations this crate writes are by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            name: "tau".to_owned(),
            email: "tau@localhost".to_owned(),
        }
    }
}

/// A handle on one jj workspace. Cheap to clone: every clone runs its
/// jobs on the same workspace, one at a time.
#[derive(Clone)]
pub struct Vcs {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    worker: Arc<Mutex<Worker>>,
}

impl std::fmt::Debug for Vcs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vcs")
            .field("root", &self.inner.root)
            .finish()
    }
}

impl Vcs {
    /// Opens the jj workspace at `dir`, which must exist.
    pub async fn open(
        dir: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let vcs = Self::lazy(dir, identity)?;
        vcs.call(|worker| worker.workspace().map(|_| ())).await?;
        Ok(vcs)
    }

    /// Makes `dir` (created if missing) a new jj repository with an
    /// internal Git store, as `jj git init` does without `--colocate`,
    /// and opens it.
    pub async fn init(
        dir: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let vcs = Self::lazy(dir, identity)?;
        vcs.call(|worker| {
            std::fs::create_dir_all(&worker.root).map_err(|source| {
                VcsError::Create {
                    path: worker.root.clone(),
                    source,
                }
            })?;
            let (workspace, _repo) = block_on(Workspace::init_internal_git(
                &worker.settings,
                &worker.root,
                // jj-lib's gix's kind, SHA-1 by default: named this way,
                // the call builds even when our gix is not jj-lib's.
                Default::default(),
            ))?;
            worker.workspace = Some(workspace);
            Ok(())
        })
        .await?;
        Ok(vcs)
    }

    /// A handle on the workspace that will be at `dir`, loaded by the
    /// first job. For a workspace a plugin creates when its run starts,
    /// after the tools that use it were built.
    pub fn lazy(
        dir: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        Self::with(dir.into(), identity, None)
    }

    /// A handle on `workspace`, loaded already at `dir` by a job.
    pub(crate) fn loaded(
        dir: PathBuf,
        identity: Identity,
        workspace: Workspace,
    ) -> Result<Self, VcsError> {
        Self::with(dir, identity, Some(workspace))
    }

    /// Opens the jj workspace at `dir` from inside a job, which may
    /// block.
    pub(crate) fn open_in_job(
        dir: PathBuf,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let settings = settings(&identity)?;
        let workspace = load(&settings, &dir)?;
        Self::loaded(dir, identity, workspace)
    }

    /// Commits whatever `@` holds, starts an empty working copy on top,
    /// and points the local bookmark `bookmark` at the run's newest
    /// commit, in one operation. See [`crate::Committed`].
    pub async fn commit_all(
        &self,
        message: impl Into<String>,
        bookmark: impl Into<String>,
    ) -> Result<crate::Committed, VcsError> {
        let message = message.into();
        let bookmark = bookmark.into();
        self.call(move |worker| {
            crate::ops::commit_all(worker, message, &bookmark)
        })
        .await
    }

    /// Ends a turn: snapshots `@`, which stays uncommitted, and points
    /// `bookmark` at the run's newest commit. See
    /// [`crate::TurnSnapshot`].
    pub async fn end_turn(
        &self,
        bookmark: impl Into<String>,
        since: Option<String>,
    ) -> Result<crate::TurnSnapshot, VcsError> {
        let bookmark = bookmark.into();
        self.call(move |worker| {
            crate::ops::end_turn(worker, &bookmark, since.as_deref())
        })
        .await
    }

    /// What `@` holds, after a snapshot.
    pub async fn working_copy(
        &self,
    ) -> Result<crate::ops::WorkingCopy, VcsError> {
        self.call(crate::ops::working_copy).await
    }

    /// The paths `@` holds in conflict, after a snapshot: what
    /// `vcs_status` lists as unresolved. `@` sits on the run's newest
    /// commit, so these are the conflicts its stack leaves.
    pub async fn conflicts(&self) -> Result<Vec<String>, VcsError> {
        self.call(crate::ops::conflicts).await
    }

    /// `@`'s diff against its parent, as text for a model, cut at the
    /// tools' size.
    pub async fn working_copy_diff(&self) -> Result<String, VcsError> {
        self.call(|worker| {
            crate::ops::diff(worker, None, Vec::new()).map(|report| report.text)
        })
        .await
    }

    /// Moves this run's own changes, up to `@`, onto `onto` (a full
    /// commit id in hex), and points `bookmark` at the run's newest
    /// commit there: how the main chat catches up with trunk. With
    /// `confirm` off it changes nothing and says what moving would do.
    pub async fn move_onto(
        &self,
        onto: impl Into<String>,
        bookmark: impl Into<String>,
        confirm: bool,
    ) -> Result<crate::Landing, VcsError> {
        let onto = onto.into();
        let bookmark = bookmark.into();
        self.call(move |worker| {
            crate::land::move_onto(worker, &onto, &bookmark, confirm)
        })
        .await
    }

    /// Lands a child run on this workspace's run (ADR 0009): rebases
    /// the child's changes, up to `child_head` (a full commit id in
    /// hex), onto this run's newest commit, starts this run's working
    /// copy on top, and points `bookmark`, this run's, at the new head.
    /// With `confirm` off it changes nothing and says what landing would
    /// do. Call it between this run's turns.
    pub async fn land(
        &self,
        child_head: impl Into<String>,
        bookmark: impl Into<String>,
        confirm: bool,
    ) -> Result<crate::Landing, VcsError> {
        let child_head = child_head.into();
        let bookmark = bookmark.into();
        self.call(move |worker| {
            crate::land::land(worker, &child_head, &bookmark, confirm)
        })
        .await
    }

    /// The paths the working copy (`@`) changes against its parents,
    /// after snapshotting it.
    pub async fn changes(&self) -> Result<Vec<crate::FileChange>, VcsError> {
        self.call(crate::ops::changes).await
    }

    /// The directory the workspace was opened at.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    fn with(
        root: PathBuf,
        identity: Identity,
        workspace: Option<Workspace>,
    ) -> Result<Self, VcsError> {
        let worker = Worker {
            root: root.clone(),
            settings: settings(&identity)?,
            workspace,
        };
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                worker: Arc::new(Mutex::new(worker)),
            }),
        })
    }

    /// Runs `job` on the workspace once the jobs before it are done, on
    /// tokio's blocking pool, and waits for its result.
    pub(crate) async fn call<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Worker) -> Result<T, VcsError> + Send + 'static,
    ) -> Result<T, VcsError> {
        let mut worker = self.inner.worker.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let worker = &mut *worker;
            panic::catch_unwind(AssertUnwindSafe(|| job(worker)))
                .unwrap_or_else(|payload| {
                    worker.workspace = None;
                    Err(VcsError::Panicked(panic_text(&*payload)))
                })
        })
        .await
        .map_err(|_| VcsError::Stopped)?
    }
}

/// What a job on the workspace gets.
pub(crate) struct Worker {
    root: PathBuf,
    settings: UserSettings,
    /// Loaded on first use, and dropped after a panic.
    workspace: Option<Workspace>,
}

impl Worker {
    /// The workspace, loaded if needed.
    pub(crate) fn workspace(&mut self) -> Result<&mut Workspace, VcsError> {
        if self.workspace.is_none() {
            self.workspace = Some(load(&self.settings, &self.root)?);
        }
        Ok(self.workspace.as_mut().expect("loaded above"))
    }

    /// The directory the workspace was opened at, as given.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// Loads the jj workspace at `root`. Blocks.
fn load(settings: &UserSettings, root: &Path) -> Result<Workspace, VcsError> {
    Workspace::load(
        settings,
        root,
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .map_err(|source| VcsError::NoWorkspace {
        root: root.to_path_buf(),
        source,
    })
}

/// jj's defaults, with `identity` as the user.
pub(crate) fn settings(identity: &Identity) -> Result<UserSettings, VcsError> {
    let mut config = StackedConfig::with_defaults();
    let mut user = ConfigLayer::empty(ConfigSource::User);
    user.set_value("user.name", identity.name.as_str())?;
    user.set_value("user.email", identity.email.as_str())?;
    config.add_layer(user);
    Ok(UserSettings::from_config(config)?)
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use tau_testing::block_on_io;

    use super::*;

    #[test]
    fn a_panic_becomes_an_error_and_the_workspace_reloads() {
        let dir = tempfile::tempdir().unwrap();
        block_on_io(async {
            let vcs = Vcs::init(dir.path(), Identity::default()).await.unwrap();
            let err = vcs
                .call(|_worker| -> Result<(), VcsError> { panic!("boom") })
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), "jj-lib panicked: boom");
            vcs.call(|worker| worker.workspace().map(|_| ()))
                .await
                .unwrap();
        });
    }

    #[test]
    fn open_fails_without_a_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let err = block_on_io(Vcs::open(dir.path(), Identity::default()))
            .unwrap_err();
        assert!(err.to_string().starts_with("No jj workspace at"));
    }
}
