//! Setting up jj in the person's Git repository, where it is (ADR 0043,
//! `docs/reference/vcs.md`, "Setting up a local repository"), all or
//! nothing.
//!
//! The steps are `jj git init --colocate`'s in a Git checkout (jj-cli
//! 0.46.0's `commands/git/init.rs`), with tau's operations, and with
//! one difference: the jj CLI resets Git's index in the folder after its
//! snapshot, and tau never moves Git's `HEAD` or index there. A folder
//! that is a colocated jj repository already keeps its repository as it
//! is: only tau's workspace and state are added.
//!
//! A setup that fails, or that its caller stops ([`Step`]), takes away
//! what it added, and the folder's refs, `HEAD`, index, config and files
//! are as before. Objects it wrote may stay in `.git/objects`, as
//! unreachable loose objects, until Git collects them; in a jj
//! repository of the person's, the operations and index files it wrote
//! stay too, unreachable from the operation log's heads. A setup cut off
//! by a crash leaves a mark in the `.jj` it made, so the next setup
//! takes that `.jj` away and starts again ([`unfinished`]).

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::Arc,
};

use jj_lib::{
    git::{
        GitImportOptions,
        GitRefKind,
        import_head,
        import_refs,
        parse_git_ref,
    },
    object_id::ObjectId as _,
    ref_name::{WorkspaceName, WorkspaceNameBuf},
    repo::{ReadonlyRepo, Repo as _},
    transaction::Transaction,
    workspace::Workspace,
};
use pollster::block_on;

use super::{Found, Kind};
use crate::{
    Identity,
    Layout,
    error::VcsError,
    lock::GitLock,
    main_workspace_name,
    revsets::ConfigRevsets,
    session::Immutable,
    snapshot_config::SnapshotConfig,
};

/// The folder in a local repository's `.jj/repo` that holds tau's state.
const TAU_DIR: &str = "tau";
/// tau's state, in [`TAU_DIR`] ([`State`]).
const STATE_FILE: &str = "state.json";
/// In [`TAU_DIR`] while a setup that made the `.jj` has not finished.
const MAKING_FILE: &str = "making";

/// A step of setting up a local repository, in order, for progress.
/// The `default` workspace's snapshot is skipped in a jj repository of
/// the person's: its working copy is theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// Git's branches and tags become jj's bookmarks and tags.
    ImportRefs,
    /// The files in the folder become the `default` workspace's
    /// working-copy commit, on the commit Git's `HEAD` names.
    Snapshot,
    /// The main chat's workspace is made, a Git worktree.
    Workspace,
    /// tau's state is written down ([`State`]).
    Record,
}

impl Step {
    pub const ALL: [Self; 4] = [
        Self::ImportRefs,
        Self::Snapshot,
        Self::Workspace,
        Self::Record,
    ];
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ImportRefs => "importing Git's branches and tags",
            Self::Snapshot => "snapshotting the working copy",
            Self::Workspace => "making the main chat's workspace",
            Self::Record => "recording tau's state",
        })
    }
}

/// What tau keeps about a local repository in it, in
/// `.jj/repo/tau/state.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    /// The repository's stable id: plugin data keys on it, not on the
    /// folder's path. A setup again after tau's state was kept keeps it.
    pub repo_id: String,
    /// The main chat's workspace's name ([`main_workspace_name`]).
    pub main_workspace: String,
    /// The name of the runs' directory, made unique when the repository
    /// was added ([`runs_dir_name`]) and never made again.
    pub runs_dir: String,
}

impl State {
    /// The state of the local repository at `folder`, when it was set
    /// up.
    pub fn read(folder: &Path) -> Result<Option<Self>, VcsError> {
        let path = tau_dir(folder).join(STATE_FILE);
        let text = match std::fs::read(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => {
                return Err(VcsError::LocalState {
                    path,
                    reason: error.to_string(),
                });
            }
        };
        serde_json::from_slice(&text).map(Some).map_err(|error| {
            VcsError::LocalState {
                path,
                reason: error.to_string(),
            }
        })
    }

    /// Writes the state of the repository at `folder`, whole or not at
    /// all: to a file beside it, then renamed over it.
    fn write(&self, folder: &Path) -> Result<(), VcsError> {
        let dir = tau_dir(folder);
        let path = dir.join(STATE_FILE);
        let partial = dir.join(format!("{STATE_FILE}.partial"));
        let text = serde_json::to_vec_pretty(self).expect("a state serializes");
        std::fs::write(&partial, text).map_err(|source| VcsError::Create {
            path: partial.clone(),
            source,
        })?;
        std::fs::rename(&partial, &path)
            .map_err(|source| VcsError::Create { path, source })
    }
}

/// `folder/.jj/repo/tau`.
fn tau_dir(folder: &Path) -> PathBuf {
    folder.join(".jj").join("repo").join(TAU_DIR)
}

/// Whether the jj repository at `repo_path` (a `.jj/repo`) is a local
/// repository tau set up, or is setting up: its `default` workspace is
/// the person's, and tau's writes import Git's refs first.
pub(crate) fn is_local(repo_path: &Path) -> bool {
    repo_path.join(TAU_DIR).is_dir()
}

/// Whether the `.jj` in `folder` is one a setup made and did not
/// finish, cut off by a crash: the next setup takes it away and starts
/// again, and inspecting the folder sees a Git repository.
pub fn unfinished(folder: &Path) -> bool {
    tau_dir(folder).join(MAKING_FILE).is_file()
}

/// A name for a new local repository's runs' directory under `parent`:
/// the folder's name, its characters other than letters, digits, `.`,
/// `-` and `_` made `-`, or `repo` for none, with `-2`, `-3` and so on
/// when `parent` has one by that name. The caller keeps the name: it is
/// recorded in [`State`], and never made again from the folder.
pub fn runs_dir_name(parent: &Path, folder: &Path) -> String {
    let base: String = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = base.trim_start_matches('.');
    let base = if base.is_empty() { "repo" } else { base };
    let taken = |name: &str| parent.join(name).exists();
    if !taken(base) {
        return base.to_owned();
    }
    (2_u64..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken(name))
        .expect("some number is free")
}

/// Sets jj up in the folder `found` describes, laid out as `layout` says
/// (`layout.repo_root` is the folder). Returns the main chat's
/// workspace's name. See the module's documentation.
pub(crate) fn setup(
    found: &Found,
    layout: &Layout,
    identity: &Identity,
    progress: &mut dyn FnMut(Step) -> ControlFlow<()>,
) -> Result<String, VcsError> {
    let folder = &layout.repo_root;
    if !layout.local || !crate::colocate::same_dir(folder, &found.root) {
        return Err(VcsError::FolderChanged {
            folder: found.root.clone(),
            reason: "the layout is not for this folder".to_owned(),
        });
    }
    let cleared = unfinished(folder);
    if cleared {
        clear_unfinished(folder, layout, identity)?;
    }
    let has_jj = folder.join(".jj").exists();
    if has_jj != (found.kind == Kind::Jj) && !(cleared && !has_jj) {
        return Err(VcsError::FolderChanged {
            folder: folder.clone(),
            reason: if has_jj {
                "it has a .jj now".to_owned()
            } else {
                "its .jj is gone".to_owned()
            },
        });
    }
    let mut journal = Journal::begin(layout, identity, has_jj)?;
    match journal.run(progress) {
        Ok(name) => Ok(name),
        Err(error) => match journal.undo() {
            Ok(()) => Err(error),
            Err(undo) => Err(VcsError::SetupNotUndone {
                error: Box::new(error),
                undo: Box::new(undo),
            }),
        },
    }
}

/// What a setup did, to take it away again ([`Journal::undo`]).
struct Journal<'a> {
    layout: &'a Layout,
    identity: &'a Identity,
    git_dir: PathBuf,
    /// Whether the folder had a jj repository of the person's.
    had_jj: bool,
    /// Whether the setup made the folder's `.jj`.
    made_jj: bool,
    /// Every ref the Git store had, with its target.
    refs_before: HashMap<String, String>,
    /// The folders in the Git store, its objects' aside.
    git_dirs_before: BTreeSet<PathBuf>,
    /// The branches and tags tau's exports made, as full ref names with
    /// their targets: the bookmarks an import made for remote branches
    /// the person's `auto-track-bookmarks` matches.
    exported: Vec<(String, String)>,
    /// In a jj repository of the person's: its operation heads, its
    /// workspace store's index, and whether it had tau's folder.
    op_heads_before: BTreeSet<String>,
    workspace_store_before: Option<Vec<u8>>,
    made_tau_dir: bool,
    /// The operations the setup wrote, by id in hex.
    ops: Vec<String>,
    /// What the main chat's directory and the runs' directory were.
    main_dir_was: Was,
    runs_dir_was: Was,
    /// The folders above those two that were missing, deepest first.
    missing_above: Vec<PathBuf>,
    /// The folders of the person's jj config that a repository config
    /// would be made in, `repos/` and up, that were missing.
    missing_config: Vec<PathBuf>,
}

/// What a directory was before the setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Was {
    Missing,
    Empty,
    /// Another directory tau keeps as it is: a runs' directory with runs.
    Kept,
}

impl<'a> Journal<'a> {
    fn begin(
        layout: &'a Layout,
        identity: &'a Identity,
        had_jj: bool,
    ) -> Result<Self, VcsError> {
        let git_dir = layout.git_dir();
        let repo_path = layout.repo_path();
        let main_dir_was = was(&layout.main_dir);
        if main_dir_was == Was::Kept {
            return Err(VcsError::InTheWay(layout.main_dir.clone()));
        }
        let mut journal = Self {
            layout,
            identity,
            refs_before: refs(&git_dir)?,
            git_dirs_before: git_dirs(&git_dir),
            git_dir,
            had_jj,
            made_jj: false,
            exported: Vec::new(),
            op_heads_before: BTreeSet::new(),
            workspace_store_before: None,
            made_tau_dir: false,
            ops: Vec::new(),
            main_dir_was,
            runs_dir_was: was(&layout.runs_dir),
            missing_above: missing_above(&[&layout.main_dir, &layout.runs_dir]),
            missing_config: identity
                .env
                .root_config_dir()
                .map(|root| {
                    let repos = root.join(crate::config::REPO_CONFIG_DIR);
                    let mut missing = missing_above(&[&repos]);
                    if !repos.exists() {
                        missing.insert(0, repos);
                    }
                    missing
                })
                .unwrap_or_default(),
        };
        if had_jj {
            journal.op_heads_before = op_heads(&repo_path);
            journal.workspace_store_before =
                std::fs::read(workspace_store_index(&repo_path)).ok();
        }
        Ok(journal)
    }

    /// The setup's steps. Returns the main chat's workspace's name.
    fn run(
        &mut self,
        progress: &mut dyn FnMut(Step) -> ControlFlow<()>,
    ) -> Result<String, VcsError> {
        let folder = self.layout.repo_root.clone();
        let repo_path = self.layout.repo_path();
        // What the jj CLI would run with in the folder: the import
        // options are read before anything is written, so a setting jj
        // cannot read stops the setup with nothing to undo.
        let settings =
            crate::config::settings(self.identity, &folder, &repo_path)?;
        let options = crate::colocate::import_options(&settings)?;
        let step = |progress: &mut dyn FnMut(Step) -> ControlFlow<()>,
                    step: Step| match progress(step) {
            ControlFlow::Continue(()) => Ok(()),
            ControlFlow::Break(()) => Err(VcsError::SetupStopped(step)),
        };

        step(progress, Step::ImportRefs)?;
        let (repo, lock) = if self.had_jj {
            self.make_tau_dir(false)?;
            let lock = crate::lock::lock_repo(&repo_path)?;
            let git = lock.git()?;
            let person = load(&settings, &folder)?;
            let repo = block_on(person.repo_loader().load_at_head())?;
            let repo = self.import_git(&repo, &settings, &options, &git)?;
            drop(git);
            (First::Done(repo), lock)
        } else {
            self.import_first(&settings)?
        };
        let git = lock.git()?;
        let repo = match repo {
            First::Done(repo) => repo,
            First::Snapshot(repo, workspace) => {
                step(progress, Step::Snapshot)?;
                let repo = self.snapshot(repo, *workspace, &settings, &git)?;
                let repo = self.import_git(&repo, &settings, &options, &git)?;
                write_trunk(&self.identity.env, &repo_path, &self.git_dir)?;
                repo
            }
        };

        step(progress, Step::Workspace)?;
        let name = self.add_main_workspace(&repo, &settings, &options, &git)?;
        drop(git);
        drop(lock);

        step(progress, Step::Record)?;
        let repo_id = State::read(&folder)
            .ok()
            .flatten()
            .map(|state| state.repo_id)
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let runs_dir = self
            .layout
            .runs_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        State {
            repo_id,
            main_workspace: name.clone(),
            runs_dir,
        }
        .write(&folder)?;
        let making = tau_dir(&folder).join(MAKING_FILE);
        if making.exists() {
            std::fs::remove_file(&making).map_err(|source| {
                VcsError::Delete {
                    path: making,
                    source,
                }
            })?;
        }
        Ok(name)
    }

    /// The first steps in a folder with no jj repository: jj's
    /// repository on the folder's `.git`, as `jj git init --colocate`
    /// makes it with `Workspace::init_external_git`, its `default`
    /// workspace on the root commit, and Git's refs imported, with
    /// nothing abandoned (jj-cli's `init_git_refs`). Returns the
    /// repository and the `default` workspace, and tau's lock.
    fn import_first(
        &mut self,
        settings: &jj_lib::settings::UserSettings,
    ) -> Result<(First, crate::lock::RepoLock), VcsError> {
        let folder = &self.layout.repo_root;
        let (begun, repo) = crate::workspace_init::begin_repo(
            settings,
            folder,
            crate::workspace_init::GitStore::External(&self.git_dir),
        )
        .map_err(VcsError::MakeRepo)?;
        self.made_jj = true;
        self.make_tau_dir(true)?;
        crate::colocate::ignore_jj_dir(folder)?;
        let default = WorkspaceName::DEFAULT;
        begun.list(&repo, default).map_err(VcsError::MakeRepo)?;
        let lock = crate::lock::lock_repo(&self.layout.repo_path())?;
        let git = lock.git()?;
        let mut tx = repo.start_transaction();
        block_on(
            tx.repo_mut()
                .check_out(default.to_owned(), &repo.store().root_commit()),
        )?;
        let options = GitImportOptions {
            // There are no refs to abandon yet, and maybe many commits:
            // as jj-cli's `init_git_refs`.
            abandon_unreachable_commits: false,
            record_synthetic_predecessors: false,
            ..crate::colocate::import_options(settings)?
        };
        block_on(import_refs(tx.repo_mut(), &options))
            .map_err(VcsError::ImportBranches)?;
        let repo = self.commit(tx, "import git refs", &git)?;
        drop(git);
        let workspace = begun
            .finish(&repo, default.to_owned())
            .map_err(VcsError::MakeRepo)?;
        Ok((First::Snapshot(repo, Box::new(workspace)), lock))
    }

    /// `default`'s working copy, as the jj CLI's first snapshot in the
    /// folder makes it (`maybe_snapshot`): Git's `HEAD` imported, and
    /// `@` a new commit on it (`import git head`), whose working copy is
    /// reset to it with no file written; then the files snapshotted with
    /// the person's snapshot settings. Unlike the CLI, Git's index is
    /// left as it is.
    fn snapshot(
        &mut self,
        repo: Arc<ReadonlyRepo>,
        mut workspace: Workspace,
        settings: &jj_lib::settings::UserSettings,
        git: &GitLock,
    ) -> Result<Arc<ReadonlyRepo>, VcsError> {
        let folder = self.layout.repo_root.clone();
        let default = WorkspaceName::DEFAULT;
        let mut tx = repo.start_transaction();
        tx.set_workspace_name(default);
        block_on(import_head(tx.repo_mut(), default, &folder))
            .map_err(VcsError::ImportBranches)?;
        let mut repo = repo;
        if tx.repo().has_changes() {
            if let Some(head) = tx.repo().view().git_head(default).as_normal() {
                let head = tx.repo().store().get_commit(head)?;
                block_on(tx.repo_mut().check_out(default.to_owned(), &head))?;
            }
            block_on(tx.repo_mut().rebase_descendants())?;
            repo = self.commit(tx, "import git head", git)?;
        }
        let wc = crate::session::wc_commit(&repo, default)?;
        let config = SnapshotConfig::load(
            &self.identity.env,
            settings,
            &folder,
            repo.store(),
        )?;
        let mut locked = block_on(workspace.start_working_copy_mutation())?;
        block_on(locked.locked_wc().reset(&wc))
            .map_err(|error| VcsError::SetupSnapshot(error.to_string()))?;
        let (tree, _) =
            block_on(locked.locked_wc().snapshot(&config.options()))?;
        if tree.tree_ids_and_labels() != wc.tree().tree_ids_and_labels() {
            let mut tx = repo.start_transaction();
            tx.set_is_snapshot(true);
            tx.set_workspace_name(default);
            let new_wc = crate::session::write_commit(&mut tx, |repo| {
                repo.rewrite_commit(&wc).set_tree(tree.clone())
            })?;
            tx.repo_mut()
                .set_wc_commit(default.to_owned(), new_wc.id().clone())?;
            block_on(tx.repo_mut().rebase_descendants())?;
            repo = self.commit(tx, "snapshot working copy", git)?;
        }
        block_on(locked.finish(repo.op_id().clone()))?;
        Ok(repo)
    }

    /// Imports Git's refs as before each of tau's writes
    /// ([`crate::colocate::import_git`]), in an operation of its own when
    /// they changed anything.
    fn import_git(
        &mut self,
        repo: &Arc<ReadonlyRepo>,
        settings: &jj_lib::settings::UserSettings,
        options: &GitImportOptions,
        git: &GitLock,
    ) -> Result<Arc<ReadonlyRepo>, VcsError> {
        let revsets = ConfigRevsets::new(
            settings,
            &self.layout.repo_root,
            WorkspaceName::DEFAULT,
        )?;
        let immutable = Immutable::of(repo.as_ref(), &revsets)?.expression();
        let mut tx = repo.start_transaction();
        crate::colocate::import_git(&mut tx, options, &immutable, git)?;
        if !tx.repo().has_changes() {
            return Ok(repo.clone());
        }
        self.commit(tx, "import git refs", git)
    }

    /// The main chat's workspace, as `ProjectRepo::add_workspace` makes a
    /// run's: a Git worktree of the folder's `.git` at the main chat's
    /// directory first, then the jj workspace there, on a new commit on
    /// the root, in one operation, after Git's refs are imported again.
    fn add_main_workspace(
        &mut self,
        repo: &Arc<ReadonlyRepo>,
        settings: &jj_lib::settings::UserSettings,
        options: &GitImportOptions,
        git: &GitLock,
    ) -> Result<String, VcsError> {
        let main_dir = self.layout.main_dir.clone();
        let store = repo.loader().workspace_store().clone();
        let view = repo.view();
        let taken = |name: &str| {
            let name = WorkspaceName::new(name);
            view.get_wc_commit_id(name).is_some()
                || store.get_workspace_path(name).ok().flatten().is_some()
        };
        let name = main_workspace_name(taken);
        std::fs::create_dir_all(&main_dir).map_err(|source| {
            VcsError::Create {
                path: main_dir.clone(),
                source,
            }
        })?;
        if self.runs_dir_was == Was::Missing {
            std::fs::create_dir_all(&self.layout.runs_dir).map_err(
                |source| VcsError::Create {
                    path: self.layout.runs_dir.clone(),
                    source,
                },
            )?;
        }
        crate::worktree::link(&self.git_dir, &main_dir, &name, None)?;
        let begun = crate::workspace_init::begin_workspace(
            &main_dir,
            &self.layout.repo_path(),
        )
        .map_err(VcsError::AddWorkspace)?;
        let revsets = ConfigRevsets::new(
            settings,
            &self.layout.repo_root,
            WorkspaceName::DEFAULT,
        )?;
        let immutable = Immutable::of(repo.as_ref(), &revsets)?.expression();
        let mut tx = repo.start_transaction();
        crate::colocate::import_git(&mut tx, options, &immutable, git)?;
        let workspace = WorkspaceNameBuf::from(name.as_str());
        let wc = block_on(
            tx.repo_mut()
                .check_out(workspace.clone(), &repo.store().root_commit()),
        )?;
        self.note_exports(&tx);
        let repo = crate::colocate::commit_in(
            tx,
            format!("add workspace {name}"),
            git,
            &workspace,
            &main_dir,
        )?;
        self.ops.push(repo.op_id().hex());
        let mut made = begun
            .finish(&repo, workspace)
            .map_err(VcsError::AddWorkspace)?;
        block_on(made.check_out(repo.op_id().clone(), None, &wc))
            .map_err(VcsError::CheckOut)?;
        crate::colocate::ignore_jj_dir(&main_dir)?;
        Ok(name)
    }

    /// [`crate::colocate::commit`], with the operation and what its
    /// export makes noted, to undo.
    fn commit(
        &mut self,
        tx: Transaction,
        what: &str,
        git: &GitLock,
    ) -> Result<Arc<ReadonlyRepo>, VcsError> {
        self.note_exports(&tx);
        let repo = crate::colocate::commit(tx, what, git)?;
        self.ops.push(repo.op_id().hex());
        Ok(repo)
    }

    /// Notes the branches and tags `tx`'s export will make in Git: the
    /// bookmarks and tags it made that Git did not have before the setup.
    fn note_exports(&mut self, tx: &Transaction) {
        let base = tx.base_repo().view();
        let view = tx.repo().view();
        let bookmarks = view.local_bookmarks().filter_map(|(name, target)| {
            let id = target.as_normal()?;
            base.get_local_bookmark(name)
                .is_absent()
                .then(|| (format!("refs/heads/{}", name.as_str()), id.hex()))
        });
        let tags = view.local_tags().filter_map(|(name, target)| {
            let id = target.as_normal()?;
            base.get_local_tag(name)
                .is_absent()
                .then(|| (format!("refs/tags/{}", name.as_str()), id.hex()))
        });
        let made: Vec<(String, String)> = bookmarks
            .chain(tags)
            .filter(|(name, _)| !self.refs_before.contains_key(name))
            .collect();
        self.exported.extend(made);
    }

    /// Makes `.jj/repo/tau`, with the mark of an unfinished setup in it
    /// when `making`: the setup made the `.jj`.
    fn make_tau_dir(&mut self, making: bool) -> Result<(), VcsError> {
        let dir = tau_dir(&self.layout.repo_root);
        if !dir.exists() {
            std::fs::create_dir(&dir).map_err(|source| VcsError::Create {
                path: dir.clone(),
                source,
            })?;
            self.made_tau_dir = true;
        }
        if making {
            let path = dir.join(MAKING_FILE);
            std::fs::write(&path, "").map_err(|source| VcsError::Create {
                path: path.clone(),
                source,
            })?;
        }
        Ok(())
    }

    /// Takes away what the setup added. The folder's refs, `HEAD`,
    /// index, config and files are then as before.
    fn undo(&self) -> Result<(), VcsError> {
        let folder = &self.layout.repo_root;
        let repo_path = self.layout.repo_path();
        // The worktree goes first: it names the main chat's directory.
        crate::worktree::unlink(&self.git_dir, &self.layout.main_dir)?;
        match self.main_dir_was {
            Was::Missing => remove_dir_all(&self.layout.main_dir)?,
            Was::Empty => empty_dir(&self.layout.main_dir)?,
            Was::Kept => {}
        }
        if self.runs_dir_was == Was::Missing {
            remove_dir_all(&self.layout.runs_dir)?;
        }
        for dir in &self.missing_above {
            // Only when empty: anything else in it is not the setup's.
            let _ = std::fs::remove_dir(dir);
        }
        self.undo_refs()?;
        self.undo_git_dirs();
        if self.made_jj {
            remove_repo_config(&self.identity.env, &repo_path)?;
            for dir in &self.missing_config {
                let _ = std::fs::remove_dir(dir);
            }
            remove_dir_all(&folder.join(".jj"))?;
            return Ok(());
        }
        // The person's jj repository: its operation heads and workspace
        // store as they were.
        let heads = repo_path.join("op_heads").join("heads");
        for op in &self.ops {
            remove_file(&heads.join(op))?;
        }
        for op in &self.op_heads_before {
            let path = heads.join(op);
            if !path.exists() {
                std::fs::write(&path, "")
                    .map_err(|source| VcsError::Create { path, source })?;
            }
        }
        let index = workspace_store_index(&repo_path);
        match &self.workspace_store_before {
            Some(bytes) => std::fs::write(&index, bytes).map_err(|source| {
                VcsError::Create {
                    path: index.clone(),
                    source,
                }
            })?,
            None => remove_file(&index)?,
        }
        if self.made_tau_dir {
            remove_dir_all(&tau_dir(folder))?;
        }
        Ok(())
    }

    /// Removes the folders the setup's refs and worktree left empty in
    /// the Git store (`refs/jj/keep/`, `worktrees/`), deepest first. One
    /// that is not empty stays: something else put a file there.
    fn undo_git_dirs(&self) {
        let mut made: Vec<PathBuf> = git_dirs(&self.git_dir)
            .into_iter()
            .filter(|dir| !self.git_dirs_before.contains(dir))
            .collect();
        made.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for dir in made {
            let _ = std::fs::remove_dir(&dir);
        }
    }

    /// Deletes the refs the setup made: jj's own under `refs/jj/`, and
    /// the branches and tags its exports made, while they are where the
    /// export put them.
    fn undo_refs(&self) -> Result<(), VcsError> {
        let now = refs(&self.git_dir)?;
        let made = now.iter().filter(|(name, target)| {
            !self.refs_before.contains_key(*name)
                && (name.starts_with("refs/jj/")
                    || self.exported.iter().any(|(exported, id)| {
                        exported == *name && id == *target
                    }))
        });
        let names: Vec<&String> = made.map(|(name, _)| name).collect();
        delete_refs(&self.git_dir, &names)
    }
}

/// What the first steps leave for the next.
enum First {
    /// The person's repository, imported.
    Done(Arc<ReadonlyRepo>),
    /// A new repository, and its `default` workspace to snapshot.
    Snapshot(Arc<ReadonlyRepo>, Box<Workspace>),
}

/// Takes away the `.jj` an unfinished setup made in `folder`
/// ([`unfinished`]), with what it may have added around it: jj's refs
/// (the folder had no jj repository, so all of `refs/jj/` is tau's), the
/// repository's jj config, and the main chat's workspace and worktree
/// when `layout`'s directory holds a workspace of that repository.
fn clear_unfinished(
    folder: &Path,
    layout: &Layout,
    identity: &Identity,
) -> Result<(), VcsError> {
    let repo_path = folder.join(".jj").join("repo");
    let git_dir = layout.git_dir();
    if layout.main_dir.join(".jj").exists() {
        let points_here =
            std::fs::read_to_string(layout.main_dir.join(".jj").join("repo"))
                .ok()
                .map(|text| layout.main_dir.join(".jj").join(text.trim()))
                .is_some_and(|path| {
                    crate::colocate::same_dir(&path, &repo_path)
                });
        if points_here {
            crate::worktree::unlink(&git_dir, &layout.main_dir)?;
            empty_dir(&layout.main_dir)?;
        }
    }
    let jj_refs: Vec<String> = refs(&git_dir)?
        .into_keys()
        .filter(|name| name.starts_with("refs/jj/"))
        .collect();
    delete_refs(&git_dir, &jj_refs.iter().collect::<Vec<_>>())?;
    remove_repo_config(&identity.env, &repo_path)?;
    remove_dir_all(&folder.join(".jj"))
}

/// Sets the repository's `trunk()` to the default branch of `upstream`,
/// else of `origin`, as `jj git init` does in a Git checkout (jj-cli
/// 0.46.0's `maybe_set_repository_level_trunk_alias`): the first of the
/// two with a `refs/remotes/<remote>/HEAD` decides, and when that names
/// a branch, `trunk()` is `<branch>@<remote>`.
fn write_trunk(
    env: &crate::JjEnv,
    repo_path: &Path,
    git_dir: &Path,
) -> Result<(), VcsError> {
    let repo =
        crate::clone::open(git_dir).map_err(|source| VcsError::NoGitStore {
            root: git_dir.to_owned(),
            source,
        })?;
    for remote in ["upstream", "origin"] {
        let Some(head) = repo.try_find_reference(
            format!("refs/remotes/{remote}/HEAD").as_str(),
        )?
        else {
            continue;
        };
        if let Some(target) = head.target().try_name()
            && let Ok(target) = std::str::from_utf8(target.as_bstr())
            && let Some((GitRefKind::Bookmark, symbol)) =
                parse_git_ref(target.as_ref())
        {
            let symbol = symbol.name.to_remote_symbol(remote.as_ref());
            crate::config::write_repo_trunk(
                env,
                repo_path,
                &symbol.to_string(),
            )?;
        }
        return Ok(());
    }
    Ok(())
}

/// Removes the jj config directory of the repository at `repo_path`, as
/// jj-lib keeps it in the person's config directory, when it has one.
fn remove_repo_config(
    env: &crate::JjEnv,
    repo_path: &Path,
) -> Result<(), VcsError> {
    let Some(root) = env.root_config_dir() else {
        return Ok(());
    };
    let Ok(id) = std::fs::read_to_string(repo_path.join("config-id")) else {
        return Ok(());
    };
    let dir = root.join(crate::config::REPO_CONFIG_DIR).join(id.trim());
    match jj_lib::secure_config::remove_repo_config_dir(&dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(VcsError::Delete { path: dir, source }),
    }
}

/// Every ref of the Git store at `git_dir`, by full name, with its
/// target: a commit id in hex, or the name a symbolic ref points at.
fn refs(git_dir: &Path) -> Result<HashMap<String, String>, VcsError> {
    let repo =
        crate::clone::open(git_dir).map_err(|source| VcsError::NoGitStore {
            root: git_dir.to_owned(),
            source,
        })?;
    let platform = repo.references()?;
    let mut found = HashMap::new();
    for reference in platform.all()? {
        let reference = reference.map_err(|error| {
            VcsError::GitConfig(format!("Cannot read Git's refs: {error}"))
        })?;
        let target = match reference.target() {
            gix::refs::TargetRef::Object(id) => id.to_string(),
            gix::refs::TargetRef::Symbolic(name) => name.as_bstr().to_string(),
        };
        found.insert(reference.name().as_bstr().to_string(), target);
    }
    Ok(found)
}

/// Deletes the refs `names` from the Git store at `git_dir`.
fn delete_refs(git_dir: &Path, names: &[&String]) -> Result<(), VcsError> {
    if names.is_empty() {
        return Ok(());
    }
    use gix::refs::transaction::{Change, PreviousValue, RefEdit, RefLog};
    let repo =
        crate::clone::open(git_dir).map_err(|source| VcsError::NoGitStore {
            root: git_dir.to_owned(),
            source,
        })?;
    let edits = names
        .iter()
        .map(|name| -> Result<RefEdit, VcsError> {
            Ok(RefEdit {
                change: Change::Delete {
                    expected: PreviousValue::Any,
                    log: RefLog::AndReference,
                },
                name: name.as_str().try_into().map_err(|error| {
                    VcsError::GitConfig(format!("Bad ref name {name}: {error}"))
                })?,
                deref: false,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    repo.edit_references(edits)?;
    Ok(())
}

/// The folders under the Git store at `git_dir`, its objects' aside.
fn git_dirs(git_dir: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut pending = vec![git_dir.to_owned()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && path != git_dir.join("objects")
            {
                found.insert(path.clone());
                pending.push(path);
            }
        }
    }
    found
}

/// The names of the operation heads of the jj repository at
/// `repo_path`.
fn op_heads(repo_path: &Path) -> BTreeSet<String> {
    std::fs::read_dir(repo_path.join("op_heads").join("heads"))
        .map(|entries| {
            entries
                .filter_map(|entry| {
                    let name = entry.ok()?.file_name();
                    let name = name.to_str()?.to_owned();
                    (name != "lock").then_some(name)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The file jj-lib's workspace store keeps its workspaces' paths in.
fn workspace_store_index(repo_path: &Path) -> PathBuf {
    repo_path.join("workspace_store").join("index")
}

/// The workspace at `dir`, loaded with `settings`.
fn load(
    settings: &jj_lib::settings::UserSettings,
    dir: &Path,
) -> Result<Workspace, VcsError> {
    Workspace::load(
        settings,
        dir,
        &jj_lib::default_backend_factories::default_backend_factories(),
        &jj_lib::default_backend_factories::default_working_copy_factories(),
    )
    .map_err(|source| VcsError::NoWorkspace {
        root: dir.to_owned(),
        source,
    })
}

/// The missing folders above each of `dirs`, deepest first.
fn missing_above(dirs: &[&Path]) -> Vec<PathBuf> {
    let mut missing: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|dir| dir.ancestors().skip(1))
        .filter(|dir| !dir.as_os_str().is_empty() && !dir.exists())
        .map(Path::to_owned)
        .collect();
    missing.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    missing.dedup();
    missing
}

/// What `dir` is now.
fn was(dir: &Path) -> Was {
    if !dir.exists() {
        return Was::Missing;
    }
    match std::fs::read_dir(dir) {
        Ok(mut entries) => match entries.next() {
            None => Was::Empty,
            Some(_) => Was::Kept,
        },
        Err(_) => Was::Kept,
    }
}

fn remove_dir_all(dir: &Path) -> Result<(), VcsError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if gone(&error) => Ok(()),
        Err(source) => Err(VcsError::Delete {
            path: dir.to_owned(),
            source,
        }),
    }
}

fn remove_file(path: &Path) -> Result<(), VcsError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if gone(&error) => Ok(()),
        Err(source) => Err(VcsError::Delete {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Whether `error` says there is nothing at the path: not there, or
/// under a file.
fn gone(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Empties `dir`, which stays.
fn empty_dir(dir: &Path) -> Result<(), VcsError> {
    remove_dir_all(dir)?;
    std::fs::create_dir_all(dir).map_err(|source| VcsError::Create {
        path: dir.to_owned(),
        source,
    })
}
