//! [`Vcs`]: a handle on one jj workspace, and the thread that owns it
//! (`docs/reference/vcs.md`, "Threading").
//!
//! jj-lib's futures are not `Send`, and much of their work is blocking
//! file and object I/O. So one thread per [`Vcs`] owns the workspace and
//! runs every job, in order, driving jj-lib's futures with `pollster`.
//! A job that panics is caught there: its caller gets an error, and the
//! workspace is loaded again for the next job, since jj-lib's state
//! after a panic cannot be trusted.

use std::{
    any::Any,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
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
use tokio::sync::oneshot;

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

/// A handle on one jj workspace. Cheap to clone: every clone sends its
/// jobs to the same thread, which stops when the last clone is dropped.
#[derive(Clone)]
pub struct Vcs {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    jobs: mpsc::Sender<Job>,
}

type Job = Box<dyn FnOnce(&mut Worker) + Send>;

impl std::fmt::Debug for Vcs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vcs")
            .field("root", &self.inner.root)
            .finish()
    }
}

impl Vcs {
    /// Opens the jj workspace at `dir`, which must exist. Blocks while
    /// the workspace loads.
    pub fn open(
        dir: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let vcs = Self::spawn(dir.into(), identity)?;
        vcs.call_blocking(|worker| worker.workspace().map(|_| ()))?;
        Ok(vcs)
    }

    /// Makes `dir` (created if missing) a new jj repository with an
    /// internal Git store, as `jj git init` does without `--colocate`,
    /// and opens it. Blocks while the repository is written.
    pub fn init(
        dir: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|source| VcsError::Create {
            path: dir.clone(),
            source,
        })?;
        let vcs = Self::spawn(dir, identity)?;
        vcs.call_blocking(|worker| {
            let (workspace, _repo) = block_on(Workspace::init_internal_git(
                &worker.settings,
                &worker.root,
                // jj-lib's gix's kind, SHA-1 by default: named this way,
                // the call builds even when our gix is not jj-lib's.
                Default::default(),
            ))?;
            worker.workspace = Some(workspace);
            Ok(())
        })?;
        Ok(vcs)
    }

    /// A handle on the workspace that will be at `dir`, loaded by the
    /// first job. For a workspace a plugin creates when its run starts,
    /// after the tools that use it were built.
    pub fn lazy(
        dir: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        Self::spawn(dir.into(), identity)
    }

    /// Ends a turn: commits what it changed, if anything, starts an
    /// empty working copy on top, and points the local bookmark
    /// `bookmark` at the run's newest commit, all in one operation. See
    /// [`crate::TurnCommit`].
    pub async fn checkpoint(
        &self,
        message: impl Into<String>,
        bookmark: impl Into<String>,
    ) -> Result<crate::TurnCommit, VcsError> {
        let message = message.into();
        let bookmark = bookmark.into();
        self.call(move |worker| {
            crate::ops::checkpoint(worker, message, &bookmark)
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

    fn spawn(root: PathBuf, identity: Identity) -> Result<Self, VcsError> {
        let settings = settings(&identity)?;
        let (jobs, receiver) = mpsc::channel::<Job>();
        let thread_root = root.clone();
        std::thread::Builder::new()
            .name("tau-vcs".to_owned())
            .spawn(move || {
                let mut worker = Worker {
                    root: thread_root,
                    settings,
                    workspace: None,
                };
                while let Ok(job) = receiver.recv() {
                    job(&mut worker);
                }
            })
            .map_err(VcsError::Thread)?;
        Ok(Self {
            inner: Arc::new(Inner { root, jobs }),
        })
    }

    fn submit<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Worker) -> Result<T, VcsError> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<T, VcsError>>, VcsError> {
        let (reply, receiver) = oneshot::channel();
        let job: Job = Box::new(move |worker| {
            let result = panic::catch_unwind(AssertUnwindSafe(|| job(worker)))
                .unwrap_or_else(|payload| {
                    worker.workspace = None;
                    Err(VcsError::Panicked(panic_text(&*payload)))
                });
            let _ = reply.send(result);
        });
        self.inner.jobs.send(job).map_err(|_| VcsError::Stopped)?;
        Ok(receiver)
    }

    /// Runs `job` on the workspace's thread and waits for its result.
    pub(crate) async fn call<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Worker) -> Result<T, VcsError> + Send + 'static,
    ) -> Result<T, VcsError> {
        self.submit(job)?.await.map_err(|_| VcsError::Stopped)?
    }

    fn call_blocking<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Worker) -> Result<T, VcsError> + Send + 'static,
    ) -> Result<T, VcsError> {
        self.submit(job)?
            .blocking_recv()
            .map_err(|_| VcsError::Stopped)?
    }
}

/// What the workspace's thread owns.
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
            let workspace = Workspace::load(
                &self.settings,
                &self.root,
                &default_backend_factories(),
                &default_working_copy_factories(),
            )
            .map_err(|source| VcsError::NoWorkspace {
                root: self.root.clone(),
                source,
            })?;
            self.workspace = Some(workspace);
        }
        Ok(self.workspace.as_mut().expect("loaded above"))
    }

    /// The directory the workspace was opened at, as given.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
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
impl Vcs {
    /// Panics on the workspace's thread, to test that the panic is
    /// caught and the workspace reloaded.
    pub(crate) fn panic_for_tests(&self) -> Result<(), VcsError> {
        self.call_blocking(|_worker| -> Result<(), VcsError> { panic!("boom") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_an_error_and_the_workspace_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let vcs = Vcs::init(dir.path(), Identity::default()).unwrap();
        let err = vcs.panic_for_tests().unwrap_err();
        assert_eq!(err.to_string(), "jj-lib panicked: boom");
        vcs.call_blocking(|worker| worker.workspace().map(|_| ()))
            .unwrap();
    }

    #[test]
    fn open_fails_without_a_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let err = Vcs::open(dir.path(), Identity::default()).unwrap_err();
        assert!(err.to_string().starts_with("No jj workspace at"));
    }
}
