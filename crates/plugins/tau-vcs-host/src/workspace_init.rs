//! Making repositories and workspaces as jj-lib's `Workspace::init_*`
//! and `Workspace::init_workspace_with_existing_repo` make them, but
//! without the operation they write (`add workspace '<name>'`, which
//! carries no tag of tau's, `crate::operations`): the caller's own
//! transaction adds the workspace's working-copy commit, and the
//! workspace's working copy starts at the operation that wrote.
//!
//! The steps are jj-lib 0.46.0's (`workspace.rs`): [`begin_repo`] or
//! [`begin_workspace`] makes `.jj` up to that operation, the caller
//! commits its transaction, and [`Begun::finish`] makes the working
//! copy and lists the workspace in the repository's workspace store.

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use jj_lib::{
    backend::Backend,
    default_backend_factories::default_working_copy_factory,
    file_util::{self, IoResultExt as _, PathError},
    git_backend::GitBackend,
    ref_name::WorkspaceNameBuf,
    repo::{ReadonlyRepo, Repo as _, RepoInitError},
    settings::UserSettings,
    signing_factory::signer_from_settings,
    workspace::{Workspace, WorkspaceInitError},
};
use pollster::block_on;

/// Where a new repository keeps its Git store.
pub(crate) enum GitStore<'a> {
    /// In `.jj/repo/store/git`, as `jj git init` makes it.
    Internal,
    /// `.git` beside `.jj`, as `jj git init --colocate` makes it.
    Colocated,
    /// The Git directory at this path.
    External(&'a Path),
}

/// A workspace's `.jj` made up to the operation that adds it.
pub(crate) struct Begun {
    workspace_root: PathBuf,
    jj_dir: PathBuf,
    /// The repository's `.jj/repo`, canonical.
    repo_path: PathBuf,
}

/// Makes a new repository at `workspace_root`, which must exist, with
/// the Git store `git`. The repository comes back at its root
/// operation, with no workspace in its view; its `.jj` is removed when
/// this fails.
pub(crate) fn begin_repo(
    settings: &UserSettings,
    workspace_root: &Path,
    git: GitStore<'_>,
) -> Result<(Begun, Arc<ReadonlyRepo>), WorkspaceInitError> {
    let jj_dir = create_jj_dir(workspace_root)?;
    let made = (|| {
        let repo_dir = jj_dir.join("repo");
        std::fs::create_dir(&repo_dir).context(&repo_dir)?;
        let backend = |settings: &UserSettings,
                       store_path: &Path|
         -> Result<Box<dyn Backend>, _> {
            Ok(Box::new(match git {
                GitStore::Internal => GitBackend::init_internal(
                    settings,
                    store_path,
                    // jj-lib's gix's kind, SHA-1 by default.
                    Default::default(),
                )?,
                GitStore::Colocated => {
                    let root = match std::fs::canonicalize(workspace_root) {
                        Ok(root) => file_util::relative_path(store_path, &root),
                        Err(_) => workspace_root.to_owned(),
                    };
                    GitBackend::init_colocated(
                        settings,
                        store_path,
                        &root,
                        Default::default(),
                    )?
                }
                GitStore::External(git_dir) => {
                    // A Git directory inside the workspace is kept by a
                    // relative path, so the workspace can move.
                    let path = match (
                        std::fs::canonicalize(workspace_root),
                        jj_lib::git_backend::canonicalize_git_repo_path(
                            git_dir,
                        ),
                    ) {
                        (Ok(root), Ok(git_dir))
                            if git_dir.starts_with(&root) =>
                        {
                            file_util::relative_path(store_path, &git_dir)
                        }
                        _ => git_dir.to_owned(),
                    };
                    GitBackend::init_external(settings, store_path, &path)?
                }
            }))
        };
        let repo = block_on(ReadonlyRepo::init(
            settings,
            &repo_dir,
            &backend,
            signer_from_settings(settings)?,
            ReadonlyRepo::default_workspace_store_initializer(),
            ReadonlyRepo::default_op_store_initializer(),
            ReadonlyRepo::default_op_heads_store_initializer(),
            ReadonlyRepo::default_index_store_initializer(),
            ReadonlyRepo::default_submodule_store_initializer(),
        ))
        .map_err(|error| match error {
            RepoInitError::Backend(error) => WorkspaceInitError::Backend(error),
            RepoInitError::OpHeadsStore(error) => {
                WorkspaceInitError::OpHeadsStore(error)
            }
            RepoInitError::Path(error) => WorkspaceInitError::Path(error),
        })?;
        let repo_path = std::fs::canonicalize(&repo_dir).context(&repo_dir)?;
        Ok((
            Begun {
                workspace_root: workspace_root.to_owned(),
                jj_dir: jj_dir.clone(),
                repo_path,
            },
            repo,
        ))
    })();
    if made.is_err() {
        let _ = std::fs::remove_dir_all(&jj_dir);
    }
    made
}

/// Makes `.jj` at `workspace_root`, which must exist, for a new
/// workspace of the repository at `repo_path` (another workspace's
/// `.jj/repo`).
pub(crate) fn begin_workspace(
    workspace_root: &Path,
    repo_path: &Path,
) -> Result<Begun, WorkspaceInitError> {
    let jj_dir = create_jj_dir(workspace_root)?;
    let repo_path = std::fs::canonicalize(repo_path).context(repo_path)?;
    let jj_dir_abs = std::fs::canonicalize(&jj_dir).context(&jj_dir)?;
    let to_store = file_util::relative_path(&jj_dir_abs, &repo_path);
    let to_store = if to_store.is_relative() {
        file_util::slash_path(&to_store).into_owned()
    } else {
        to_store
    };
    let bytes = file_util::path_to_bytes(&to_store)
        .map_err(WorkspaceInitError::EncodeRepoPath)?;
    let file = jj_dir.join("repo");
    std::fs::write(&file, bytes).context(&file)?;
    Ok(Begun {
        workspace_root: workspace_root.to_owned(),
        jj_dir,
        repo_path,
    })
}

impl Begun {
    /// The workspace `name`, whose working-copy commit `repo`'s
    /// operation added: its working copy starts at that operation with
    /// no files, and the workspace store lists it. The caller checks the
    /// commit out.
    pub(crate) fn finish(
        self,
        repo: &ReadonlyRepo,
        name: WorkspaceNameBuf,
    ) -> Result<Workspace, WorkspaceInitError> {
        let state = self.jj_dir.join("working_copy");
        std::fs::create_dir(&state).context(&state)?;
        let factory = default_working_copy_factory();
        let working_copy = factory.init_working_copy(
            repo.store().clone(),
            self.workspace_root.clone(),
            state.clone(),
            repo.op_id().clone(),
            name,
            repo.settings(),
        )?;
        let kind = state.join("type");
        std::fs::write(&kind, working_copy.name()).context(&kind)?;
        let workspace = Workspace::new(
            &self.workspace_root,
            self.repo_path,
            working_copy,
            repo.loader().clone(),
        )?;
        repo.loader()
            .workspace_store()
            .add(workspace.workspace_name(), workspace.workspace_root())?;
        Ok(workspace)
    }
}

/// `workspace_root/.jj`, made; refused when it exists.
fn create_jj_dir(workspace_root: &Path) -> Result<PathBuf, WorkspaceInitError> {
    let jj_dir = workspace_root.join(".jj");
    match std::fs::create_dir(&jj_dir) {
        Ok(()) => Ok(jj_dir),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(WorkspaceInitError::DestinationExists(jj_dir))
        }
        Err(source) => Err(PathError {
            path: jj_dir,
            source,
        }
        .into()),
    }
}
