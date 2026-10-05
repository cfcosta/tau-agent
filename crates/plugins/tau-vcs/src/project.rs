//! [`ProjectRepo`]: a repository tau owns, with a jj workspace per run
//! (`docs/reference/vcs.md`, "Projects").
//!
//! A project lives in a directory of its own, usually under
//! `$XDG_DATA_HOME/tau/repos/`:
//!
//! - `git/`: a bare copy of the source's Git store, which jj writes to;
//! - `main/`: the jj repository, whose own working copy stays empty;
//! - `runs/<name>/`: one jj workspace per run, each on its own commit.
//!
//! Runs never touch the user's own checkout. [`ProjectRepo`]'s functions
//! block; async code reaches them through [`Project`], which runs each
//! job in `spawn_blocking`, one at a time per repository (ADR 0027).

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0027)"
)]

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use jj_lib::{
    backend::{ChangeId, CommitId},
    commit::Commit,
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
        default_working_copy_factory,
    },
    git::{GitImportOptions, REMOTE_NAME_FOR_LOCAL_GIT_REPO, import_refs},
    matchers::EverythingMatcher,
    object_id::ObjectId as _,
    ref_name::{RefName, WorkspaceNameBuf},
    repo::{ReadonlyRepo, Repo as _},
    settings::UserSettings,
    workspace::Workspace,
};
use pollster::block_on;

use crate::{
    ChangeKind,
    FileChange,
    error::VcsError,
    run_workspace::Link,
    vcs::{Identity, Vcs, settings},
};

mod push;

pub use push::{Pushed, REMOTE, Remote};

const GIT: &str = "git";
const MAIN: &str = "main";

/// How many operations back [`ProjectRepo::landed`] looks for a landing.
pub const LANDING_LOOKBACK: usize = 1000;

/// jj's own workspace: the repository's checkout, under `main/`. A
/// repository's main chat works in it; runs get workspaces of their own.
pub const DEFAULT_WORKSPACE: &str = "default";
const RUNS: &str = "runs";

/// A repository tau owns. Cheap to clone.
#[derive(Clone)]
pub struct ProjectRepo {
    inner: Arc<Inner>,
}

/// A project, for async code: each job on it runs in tokio's
/// `spawn_blocking`, after the jobs on the same repository that asked
/// before it (ADR 0027). Cheap to clone; every handle on one repository,
/// however it was made, shares its turn.
#[derive(Clone)]
pub struct Project {
    repo: ProjectRepo,
    turn: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.repo.fmt(f)
    }
}

impl From<ProjectRepo> for Project {
    fn from(repo: ProjectRepo) -> Self {
        let turn = turn_of(repo.root());
        Self { repo, turn }
    }
}

impl Project {
    /// Opens the project at `root`. See [`ProjectRepo::open`].
    pub async fn open(
        root: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let root = root.into();
        Self::make(move || ProjectRepo::open(root, identity)).await
    }

    /// Makes a project from a local repository. See
    /// [`ProjectRepo::import`].
    pub async fn import(
        source: impl Into<String>,
        root: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let (source, root) = (source.into(), root.into());
        Self::make(move || ProjectRepo::import(&source, root, identity)).await
    }

    /// Opens the project at `root`, or makes it there. See
    /// [`ProjectRepo::open_or_import`].
    pub async fn open_or_import(
        source: impl Into<String>,
        root: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let (source, root) = (source.into(), root.into());
        Self::make(move || ProjectRepo::open_or_import(&source, root, identity))
            .await
    }

    async fn make(
        open: impl FnOnce() -> Result<ProjectRepo, VcsError> + Send + 'static,
    ) -> Result<Self, VcsError> {
        tokio::task::spawn_blocking(open)
            .await
            .unwrap_or_else(|error| {
                std::panic::resume_unwind(error.into_panic())
            })
            .map(Self::from)
    }

    /// Runs `job` on the repository once the jobs before it are done, on
    /// tokio's blocking pool, and gives back what it returned. A job that
    /// panics panics here too.
    pub async fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&ProjectRepo) -> T + Send + 'static,
    ) -> T {
        let turn = self.turn.clone().lock_owned().await;
        let repo = self.repo.clone();
        tokio::task::spawn_blocking(move || {
            let _turn = turn;
            job(&repo)
        })
        .await
        .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()))
    }

    /// The repository, to call synchronously, without waiting for the
    /// jobs on it: for tests, which are synchronous, and disallowed
    /// elsewhere by `clippy.toml`.
    pub fn blocking(&self) -> &ProjectRepo {
        &self.repo
    }

    /// The project's directory.
    pub fn root(&self) -> &Path {
        self.repo.root()
    }

    /// Where run `name`'s workspace lives, whether or not it exists.
    pub fn workspace_dir(&self, name: &str) -> PathBuf {
        self.repo.workspace_dir(name)
    }
}

/// The turn every handle on the repository at `root` shares.
fn turn_of(root: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static TURNS: std::sync::LazyLock<
        std::sync::Mutex<
            HashMap<PathBuf, std::sync::Weak<tokio::sync::Mutex<()>>>,
        >,
    > = std::sync::LazyLock::new(Default::default);
    let root =
        std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut turns = TURNS.lock().expect("not poisoned");
    turns.retain(|_, turn| turn.strong_count() > 0);
    if let Some(turn) = turns.get(&root).and_then(std::sync::Weak::upgrade) {
        return turn;
    }
    let turn = Arc::new(tokio::sync::Mutex::new(()));
    turns.insert(root, Arc::downgrade(&turn));
    turn
}

struct Inner {
    root: PathBuf,
    identity: Identity,
    settings: UserSettings,
}

impl std::fmt::Debug for ProjectRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectRepo")
            .field("root", &self.inner.root)
            .finish()
    }
}

impl ProjectRepo {
    /// Opens the project at `root`, or makes it there from the local
    /// repository at `source` when there is none.
    pub fn open_or_import(
        source: &str,
        root: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let root = root.into();
        if root.join(MAIN).join(".jj").is_dir() {
            return Self::open(root, identity);
        }
        Self::import(source, root, identity)
    }

    /// Opens the project at `root`.
    pub fn open(
        root: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let root = root.into();
        if !root.join(MAIN).join(".jj").is_dir() {
            return Err(VcsError::NoProject(root));
        }
        Self::new(root, identity)
    }

    /// Makes a project at `root` (which must not hold one) from a copy of
    /// the local repository at `source`, with every branch imported as a
    /// bookmark. Needs no `git`: the object files are shared, not
    /// cloned. Cloning from a URL is not supported yet.
    pub fn import(
        source: &str,
        root: impl Into<PathBuf>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|source| VcsError::Create {
            path: root.clone(),
            source,
        })?;
        let git_dir = root.join(GIT);
        if !git_dir.is_dir() {
            copy_git_store(Path::new(source), &git_dir)?;
        }
        let project = Self::new(root, identity)?;
        let main = project.inner.root.join(MAIN);
        std::fs::create_dir_all(&main)?;
        let (_workspace, repo) = block_on(Workspace::init_external_git(
            &project.inner.settings,
            &main,
            &git_dir,
        ))
        .map_err(VcsError::MakeRepo)?;
        let mut tx = repo.start_transaction();
        let options = GitImportOptions {
            abandon_unreachable_commits: true,
            record_synthetic_predecessors: false,
            remote_auto_track_bookmarks: HashMap::new(),
        };
        block_on(import_refs(tx.repo_mut(), &options))
            .map_err(VcsError::ImportBranches)?;
        block_on(tx.commit("tau: import"))?;
        Ok(project)
    }

    fn new(root: PathBuf, identity: Identity) -> Result<Self, VcsError> {
        let settings = settings(&identity)?;
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                identity,
                settings,
            }),
        })
    }

    /// Brings in what changed at the source since the project was made
    /// or last updated: new commits, and branches where the source has
    /// them now. Trunk follows a remote's or a bare repository's new
    /// default branch, but not a checkout's checked-out branch. Runs keep their workspaces and commits; new runs start
    /// from the new trunk. Returns the trunk before and after.
    pub fn update(&self, from: UpdateFrom<'_>) -> Result<Updated, VcsError> {
        let _repo = self.lock()?;
        let before = self.trunk()?;
        let git_dir = self.inner.root.join(GIT);
        match from {
            UpdateFrom::Checkout(source) => update_git_store(source, &git_dir)?,
            UpdateFrom::Remote { url, token } => {
                crate::clone::fetch_into(&git_dir, url, token)?
            }
        }
        let repo = self.load()?;
        let mut tx = repo.start_transaction();
        let upstream = upstream_names(tx.repo().view());
        let options = GitImportOptions {
            abandon_unreachable_commits: false,
            record_synthetic_predecessors: false,
            remote_auto_track_bookmarks: HashMap::new(),
        };
        block_on(import_refs(tx.repo_mut(), &options))
            .map_err(VcsError::ImportBranches)?;
        take_upstream(&mut tx, upstream);
        block_on(tx.commit("tau: update"))?;
        Ok(Updated {
            before,
            after: self.trunk()?,
        })
    }

    /// A file's content at `commit` (a full id in hex), and whether it
    /// is executable; `None` when the commit has no file there.
    pub fn file_at(
        &self,
        commit: &str,
        path: &str,
    ) -> Result<Option<(Vec<u8>, bool)>, VcsError> {
        let repo = self.git()?;
        let id =
            gix::ObjectId::from_hex(commit.as_bytes()).map_err(|source| {
                VcsError::BadCommitHex {
                    commit: commit.to_owned(),
                    source,
                }
            })?;
        let commit = repo.find_object(id)?.try_into_commit()?;
        let mut tree = commit.tree()?;
        let Some(entry) = tree.peel_to_entry_by_path(path)? else {
            return Ok(None);
        };
        let mode = entry.mode();
        if !mode.is_blob_or_symlink() {
            return Ok(None);
        }
        let data = entry.object()?.detach().data;
        Ok(Some((data, mode.is_executable())))
    }

    /// The first parent of `commit`, if it has one.
    pub fn parent_of(&self, commit: &str) -> Result<Option<String>, VcsError> {
        let repo = self.git()?;
        let id =
            gix::ObjectId::from_hex(commit.as_bytes()).map_err(|source| {
                VcsError::BadCommitHex {
                    commit: commit.to_owned(),
                    source,
                }
            })?;
        let commit = repo.find_object(id)?.try_into_commit()?;
        Ok(commit.parent_ids().next().map(|id| id.to_string()))
    }

    /// The source's default branch, as the copy's `HEAD` names it.
    pub fn default_branch(&self) -> Option<String> {
        std::fs::read_to_string(self.inner.root.join(GIT).join("HEAD"))
            .ok()?
            .trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_owned)
    }

    fn git(&self) -> Result<gix::Repository, VcsError> {
        gix::open(self.inner.root.join(GIT)).map_err(|source| {
            VcsError::NoGitStore {
                root: self.inner.root.clone(),
                source: Box::new(source),
            }
        })
    }

    /// The project's directory.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Where run `name`'s workspace is.
    pub fn workspace_dir(&self, name: &str) -> PathBuf {
        if name == DEFAULT_WORKSPACE {
            return self.inner.root.join(MAIN);
        }
        self.inner.root.join(RUNS).join(name)
    }

    /// The commit new runs start from: the source's default branch, as
    /// the clone's `HEAD` names it, or the root commit of an empty
    /// repository. A full commit id, in hex.
    pub fn trunk(&self) -> Result<String, VcsError> {
        let repo = self.load()?;
        let id = self
            .trunk_bookmark(&repo)
            .map(|(_, id)| id)
            .unwrap_or_else(|| repo.store().root_commit_id().clone());
        Ok(id.hex())
    }

    /// The name of trunk's bookmark: the default branch, else `main`,
    /// `master` or `trunk`, whichever is set; the default branch, else
    /// `main`, when none is.
    pub fn trunk_name(&self) -> Result<String, VcsError> {
        let repo = self.load()?;
        Ok(self
            .trunk_bookmark(&repo)
            .map(|(name, _)| name)
            .unwrap_or_else(|| {
                self.default_branch().unwrap_or_else(|| "main".to_owned())
            }))
    }

    /// Trunk's bookmark and the commit it names: the default branch, else
    /// `main`, `master` or `trunk`.
    fn trunk_bookmark(
        &self,
        repo: &ReadonlyRepo,
    ) -> Option<(String, CommitId)> {
        let view = repo.view();
        self.default_branch()
            .into_iter()
            .chain(["main", "master", "trunk"].map(str::to_owned))
            .find_map(|name| {
                let id = view
                    .get_local_bookmark(RefName::new(&name))
                    .as_normal()
                    .cloned()?;
                Some((name, id))
            })
    }

    /// The changes `head` (a full commit id in hex) has that trunk lacks,
    /// oldest first: a run's stack, as a pull request replays it.
    pub fn stack(&self, head: &str) -> Result<Vec<StackChange>, VcsError> {
        let repo = self.load()?;
        let head = commit(&repo, head)?;
        let trunk =
            CommitId::try_from_hex(self.trunk()?).ok_or(VcsError::NotHex)?;
        range(&repo, head.id(), Some(&trunk))
    }

    /// Makes a fork's workspace from `snapshot`, a turn's snapshot of
    /// another run's `@` (ADR 0014): a new change with the snapshot's
    /// files, uncommitted, on the snapshot's parent as it is now. A
    /// landing may have restacked that parent since; its new files and
    /// the turn's work are then merged. When the parent's change is a
    /// workspace's working copy now (a run undid the commit), the fork
    /// starts on that parent's parent instead, with the turn's files.
    /// Opens the workspace as it is if it exists already.
    pub fn add_workspace_from_snapshot(
        &self,
        name: &str,
        snapshot: &str,
    ) -> Result<Vcs, VcsError> {
        let dir = self.workspace_dir(name);
        if dir.join(".jj").is_dir() {
            return Vcs::open_in_job(dir, self.inner.identity.clone());
        }
        std::fs::create_dir_all(&dir).map_err(|source| VcsError::Create {
            path: dir.clone(),
            source,
        })?;
        let lock = self.lock()?;
        let main = self.main()?;
        let repo = block_on(main.repo_loader().load_at_head())?;
        let (mut workspace, repo) =
            block_on(Workspace::init_workspace_with_existing_repo(
                &dir,
                main.repo_path(),
                &repo,
                &*default_working_copy_factory(),
                WorkspaceNameBuf::from(name),
            ))
            .map_err(VcsError::AddWorkspace)?;
        let turn = commit(&repo, snapshot)?;
        let then = turn.parent_ids().first().ok_or(VcsError::NoParent)?;
        let mut then = repo.store().get_commit(then)?;
        let mut now =
            visible(repo.as_ref(), &then)?.unwrap_or_else(|| then.clone());
        // An undo can take the parent's change back into a run's `@`. The
        // fork must not stand on another workspace's working copy, so it
        // starts on that parent's parent, with the turn's files: the
        // undone commit's description is not the fork's.
        let working_copies: Vec<&CommitId> =
            repo.view().wc_commit_ids().values().collect();
        if working_copies.contains(&now.id()) {
            let up = then.parent_ids().first().ok_or(VcsError::NoParent)?;
            then = repo.store().get_commit(up)?;
            let up = now.parent_ids().first().ok_or(VcsError::NoParent)?;
            now = repo.store().get_commit(up)?;
        }
        let tree = if now.id() == then.id() {
            turn.tree()
        } else {
            block_on(jj_lib::merged_tree::MergedTree::merge(
                jj_lib::merge::Merge::from_vec(vec![
                    (now.tree(), "the parent now".to_owned()),
                    (then.tree(), "the parent then".to_owned()),
                    (turn.tree(), "the turn".to_owned()),
                ]),
            ))?
        };
        let mut tx = repo.start_transaction();
        let wc = block_on(
            tx.repo_mut()
                .new_commit(vec![now.id().clone()], tree)
                .write(),
        )?;
        block_on(tx.repo_mut().edit(WorkspaceNameBuf::from(name), &wc))?;
        block_on(tx.repo_mut().rebase_descendants())?;
        let repo = block_on(tx.commit(format!("tau: add workspace {name}")))?;
        block_on(workspace.check_out(repo.op_id().clone(), None, &wc))
            .map_err(VcsError::CheckOut)?;
        drop(lock);
        Vcs::loaded(dir, self.inner.identity.clone(), workspace)
    }

    /// `links` with each `commit_id` moved to where its change is now.
    /// A change keeps its id when it is rewritten (a restack, a
    /// describe), so the stored commit may be hidden while its change
    /// lives on. A change no longer visible anywhere (abandoned) keeps
    /// its last known commit; a divergent one is an error, since its
    /// change id names more than one commit.
    pub fn current(
        &self,
        links: impl IntoIterator<Item = Link>,
    ) -> Result<Vec<Link>, VcsError> {
        let repo = self.load()?;
        links
            .into_iter()
            .map(|mut link| {
                // A turn's snapshot is that very commit: `@` has moved on
                // under the same change id.
                if link.snapshot {
                    return Ok(link);
                }
                let change = ChangeId::try_from_reverse_hex(&link.change_id)
                    .ok_or_else(|| {
                        VcsError::NotChangeId(link.change_id.clone())
                    })?;
                let visible: Vec<CommitId> =
                    match block_on(repo.resolve_change_id(&change))? {
                        Some(targets) => targets
                            .visible_with_offsets()
                            .map(|(_, id)| id.clone())
                            .collect(),
                        None => Vec::new(),
                    };
                match visible.as_slice() {
                    [] => {}
                    [id] => link.commit_id = id.hex(),
                    _ => {
                        return Err(VcsError::DivergentTurn {
                            change: link.change_id.clone(),
                            turn: link.turn,
                        });
                    }
                }
                Ok(link)
            })
            .collect()
    }

    /// The commit the local bookmark `name` points at, as a full commit
    /// id in hex. `None` when there is no such bookmark, or it has
    /// conflicting targets.
    pub fn bookmark(&self, name: &str) -> Result<Option<String>, VcsError> {
        let repo = self.load()?;
        Ok(repo
            .view()
            .get_local_bookmark(RefName::new(name))
            .as_normal()
            .map(|id| id.hex()))
    }

    /// The paths `commit` (a full commit id in hex) holds in conflict, in
    /// path order.
    pub fn conflicts(&self, commit: &str) -> Result<Vec<String>, VcsError> {
        let repo = self.load()?;
        Ok(self::commit(&repo, commit)?
            .tree()
            .conflicts()
            .map(|(path, _)| path.as_internal_file_string().to_owned())
            .collect())
    }

    /// Whether `ancestor` is `descendant` or one of its ancestors. Both
    /// are full commit ids in hex.
    pub fn is_ancestor(
        &self,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, VcsError> {
        let repo = self.load()?;
        let id = |hex: &str| {
            CommitId::try_from_hex(hex)
                .ok_or_else(|| VcsError::NotCommitId(hex.to_owned()))
        };
        Ok(block_on(
            repo.index().is_ancestor(&id(ancestor)?, &id(descendant)?),
        )?)
    }

    /// Abandons what `head` has that `keep` lacks (both full commit ids
    /// in hex): a dropped child run's own changes. Returns how many
    /// commits went. The operation log keeps them.
    pub fn abandon_between(
        &self,
        keep: &str,
        head: &str,
    ) -> Result<usize, VcsError> {
        self.abandon_beyond(&[keep], head)
    }

    /// Abandons what `head` has that none of `keeps` has (all full
    /// commit ids in hex). Returns how many commits went.
    pub fn abandon_beyond(
        &self,
        keeps: &[&str],
        head: &str,
    ) -> Result<usize, VcsError> {
        use futures_util::StreamExt as _;
        use jj_lib::revset::ResolvedRevsetExpression;

        let _repo = self.lock()?;
        let repo = self.load()?;
        let id = |hex: &str| {
            CommitId::try_from_hex(hex)
                .ok_or_else(|| VcsError::NotCommitId(hex.to_owned()))
        };
        let keeps = keeps
            .iter()
            .map(|keep| id(keep))
            .collect::<Result<Vec<_>, _>>()?;
        let ids: Vec<CommitId> = {
            let revset = ResolvedRevsetExpression::commit(id(head)?)
                .ancestors()
                .minus(&ResolvedRevsetExpression::commits(keeps).ancestors())
                .evaluate(repo.as_ref())?;
            block_on(revset.stream().collect::<Vec<_>>())
                .into_iter()
                .collect::<Result<_, _>>()?
        };
        if ids.is_empty() {
            return Ok(0);
        }
        let mut tx = repo.start_transaction();
        for id in &ids {
            let commit = repo.store().get_commit(id)?;
            tx.repo_mut().record_abandoned_commit(&commit);
        }
        block_on(tx.repo_mut().rebase_descendants())?;
        block_on(tx.commit(format!("tau: abandon {} changes", ids.len())))?;
        Ok(ids.len())
    }

    /// What the landing of the child whose head was `child_head` (a
    /// full commit id in hex, as given to `Vcs::land`) did, if one was
    /// confirmed: read back from its operation, which records it, so a
    /// host that closed between the landing and recording it can finish.
    /// Looks back over the newest [`LANDING_LOOKBACK`] operations.
    pub fn landed(
        &self,
        child_head: &str,
    ) -> Result<Option<crate::Landing>, VcsError> {
        let repo = self.load()?;
        let mut op = repo.operation().clone();
        for _ in 0..LANDING_LOOKBACK {
            if let Some(record) = op
                .metadata()
                .attributes
                .get(crate::session::LANDING_ATTRIBUTE)
                .and_then(|text| {
                    serde_json::from_str::<serde_json::Value>(text).ok()
                })
                && record["child_head"] == child_head
                && let Ok(landing) =
                    serde_json::from_value(record["landing"].clone())
            {
                return Ok(Some(landing));
            }
            // Concurrent operations merge into one: follow the first.
            match block_on(op.parents())?.into_iter().next() {
                Some(parent) => op = parent,
                None => break,
            }
        }
        Ok(None)
    }

    /// The local bookmarks whose names start with `prefix`, sorted.
    pub fn bookmarks(&self, prefix: &str) -> Result<Vec<String>, VcsError> {
        let repo = self.load()?;
        let mut names: Vec<String> = repo
            .view()
            .local_bookmarks()
            .map(|(name, _)| name.as_str().to_owned())
            .filter(|name| name.starts_with(prefix))
            .collect();
        names.sort();
        Ok(names)
    }

    /// The working-copy commit of workspace `name`, as a full commit id
    /// in hex, if the workspace exists.
    pub fn workspace_head(
        &self,
        name: &str,
    ) -> Result<Option<String>, VcsError> {
        let repo = self.load()?;
        Ok(repo
            .view()
            .get_wc_commit_id(&WorkspaceNameBuf::from(name))
            .map(|id| id.hex()))
    }

    /// Removes the local bookmark `name`, if there is one. The commits it
    /// named stay.
    pub fn remove_bookmark(&self, name: &str) -> Result<(), VcsError> {
        let _repo = self.lock()?;
        let repo = self.load()?;
        let name = RefName::new(name);
        if repo.view().get_local_bookmark(name).is_absent() {
            return Ok(());
        }
        let mut tx = repo.start_transaction();
        tx.repo_mut().set_local_bookmark_target(
            name,
            jj_lib::op_store::RefTarget::absent(),
        );
        block_on(tx.commit(format!("tau: remove bookmark {}", name.as_str())))?;
        Ok(())
    }

    /// Makes run `name`'s workspace, on a new empty commit on top of
    /// `base` (a full commit id in hex), with `base`'s files checked out,
    /// and opens it. When `base` was rewritten, the workspace starts on
    /// the commit its change names now. Opens it as it is if it exists
    /// already.
    pub fn add_workspace(
        &self,
        name: &str,
        base: &str,
    ) -> Result<Vcs, VcsError> {
        let dir = self.workspace_dir(name);
        if dir.join(".jj").is_dir() {
            return Vcs::open_in_job(dir, self.inner.identity.clone());
        }
        std::fs::create_dir_all(&dir).map_err(|source| VcsError::Create {
            path: dir.clone(),
            source,
        })?;
        let lock = self.lock()?;
        let main = self.main()?;
        let repo = block_on(main.repo_loader().load_at_head())?;
        let (mut workspace, repo) =
            block_on(Workspace::init_workspace_with_existing_repo(
                &dir,
                main.repo_path(),
                &repo,
                &*default_working_copy_factory(),
                WorkspaceNameBuf::from(name),
            ))
            .map_err(VcsError::AddWorkspace)?;
        // Where `base`'s change is now: a catch-up may have rewritten it
        // since the caller read it, and checking out the old commit would
        // bring it back beside the new one, a divergent change.
        let base = commit(&repo, base)?;
        let base = visible(repo.as_ref(), &base)?.unwrap_or(base);
        let mut tx = repo.start_transaction();
        let wc = block_on(
            tx.repo_mut().check_out(WorkspaceNameBuf::from(name), &base),
        )?;
        // Checking out abandons the empty commit the workspace began on.
        block_on(tx.repo_mut().rebase_descendants())?;
        let repo = block_on(tx.commit(format!("tau: add workspace {name}")))?;
        block_on(workspace.check_out(repo.op_id().clone(), None, &wc))
            .map_err(VcsError::CheckOut)?;
        drop(lock);
        Vcs::loaded(dir, self.inner.identity.clone(), workspace)
    }

    /// Removes run `name`'s workspace: jj forgets it, and its directory
    /// is deleted. Its commits stay in the repository. The default
    /// workspace is the repository itself, and stays.
    pub fn forget_workspace(&self, name: &str) -> Result<(), VcsError> {
        if name == DEFAULT_WORKSPACE {
            return Ok(());
        }
        let lock = self.lock()?;
        let repo = self.load()?;
        let name_buf = WorkspaceNameBuf::from(name);
        if repo.view().get_wc_commit_id(&name_buf).is_some() {
            let mut tx = repo.start_transaction();
            block_on(tx.repo_mut().remove_workspace(&name_buf))?;
            block_on(tx.repo_mut().rebase_descendants())?;
            block_on(tx.commit(format!("tau: forget workspace {name}")))?;
        }
        drop(lock);
        let dir = self.workspace_dir(name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|source| {
                VcsError::Delete {
                    path: dir.clone(),
                    source,
                }
            })?;
        }
        Ok(())
    }

    /// How the files differ from commit `from` to commit `to` (full hex
    /// ids): one entry per changed file, in path order, with its line
    /// counts and its unified diff.
    pub fn diff(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FileDiff>, VcsError> {
        let repo = self.load()?;
        let from = commit(&repo, from)?.tree();
        let to = commit(&repo, to)?.tree();
        let (text, changes) = crate::diff::unified(
            repo.as_ref(),
            &self.inner.settings,
            &from,
            &to,
            &EverythingMatcher,
        )?;
        Ok(split_files(&text, changes))
    }

    /// The names of the workspaces runs have, sorted.
    pub fn workspaces(&self) -> Result<Vec<String>, VcsError> {
        let repo = self.load()?;
        let mut names: Vec<String> = repo
            .view()
            .wc_commit_ids()
            .keys()
            .map(|name| name.as_str().to_owned())
            .filter(|name| name != "default")
            .collect();
        names.sort();
        Ok(names)
    }

    fn main(&self) -> Result<Workspace, VcsError> {
        Workspace::load(
            &self.inner.settings,
            &self.inner.root.join(MAIN),
            &default_backend_factories(),
            &default_working_copy_factories(),
        )
        .map_err(|source| VcsError::ProjectLoad {
            root: self.inner.root.clone(),
            source,
        })
    }

    /// The repository's lock, which every write takes
    /// (`crate::lock`). Reads load the repository at its newest
    /// operation and need none.
    fn lock(&self) -> Result<jj_lib::lock::FileLock, VcsError> {
        crate::lock::lock_repo(
            &self.inner.root.join(MAIN).join(".jj").join("repo"),
        )
    }

    fn load(&self) -> Result<Arc<ReadonlyRepo>, VcsError> {
        let main = self.main()?;
        Ok(block_on(main.repo_loader().load_at_head())?)
    }
}

/// Where [`ProjectRepo::update`] brings changes from.
#[derive(Debug, Clone, Copy)]
pub enum UpdateFrom<'a> {
    /// The local repository the project was imported from.
    Checkout(&'a Path),
    /// A remote, over HTTPS, with a token if it needs one.
    Remote {
        url: &'a str,
        token: Option<&'a str>,
    },
}

/// The trunk before and after an update, as full commit ids in hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updated {
    pub before: String,
    pub after: String,
}

impl Updated {
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// One file's change between two commits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileDiff {
    pub path: String,
    pub kind: ChangeKind,
    pub added: usize,
    pub removed: usize,
    /// The file's unified diff, as `git diff` writes it.
    pub text: String,
}

/// Splits a multi-file diff into its files, in the order `changes` lists
/// them, and counts each file's lines.
fn split_files(text: &str, changes: Vec<FileChange>) -> Vec<FileDiff> {
    let mut sections: Vec<&str> = Vec::new();
    let mut start = None;
    for (at, _) in text.match_indices("diff --git a/") {
        if at == 0 || text.as_bytes()[at - 1] == b'\n' {
            if let Some(begin) = start {
                sections.push(&text[begin..at]);
            }
            start = Some(at);
        }
    }
    if let Some(begin) = start {
        sections.push(&text[begin..]);
    }
    changes
        .into_iter()
        .zip(
            sections
                .into_iter()
                .map(Some)
                .chain(std::iter::repeat(None)),
        )
        .map(|(change, section)| {
            let text = section.unwrap_or_default().to_owned();
            // The `---`/`+++` header lines come before the first hunk;
            // in a hunk, `---` is a removed line that starts with `--`.
            let hunks = text.find("\n@@").map_or("", |at| &text[at + 1..]);
            let (mut added, mut removed) = (0, 0);
            for line in hunks.lines() {
                if line.starts_with('+') {
                    added += 1;
                } else if line.starts_with('-') {
                    removed += 1;
                }
            }
            FileDiff {
                path: change.path,
                kind: change.kind,
                added,
                removed,
                text,
            }
        })
        .collect()
}

/// The bookmarks the source has, as the last import left them.
fn upstream_names(view: &jj_lib::view::View) -> HashSet<String> {
    view.remote_bookmarks(REMOTE_NAME_FOR_LOCAL_GIT_REPO)
        .map(|(name, _)| name.as_str().to_owned())
        .collect()
}

/// Points every bookmark of the source's that an update left with two
/// targets where the source has it now, and deletes it when the source
/// deleted it. The main chat moves trunk here (ADR 0015) while upstream
/// moves it there, or renames or deletes the branch. The main chat's
/// commits stay on its `@`, and go onto trunk when it catches up
/// (`Vcs::move_onto`). `before` is the source's bookmarks before the
/// import.
fn take_upstream(
    tx: &mut jj_lib::transaction::Transaction,
    before: HashSet<String>,
) {
    let names = before.into_iter().chain(upstream_names(tx.repo().view()));
    for name in names.collect::<HashSet<_>>() {
        let name = RefName::new(&name);
        let view = tx.repo().view();
        if !view.get_local_bookmark(name).has_conflict() {
            continue;
        }
        let upstream = view
            .get_remote_bookmark(
                name.to_remote_symbol(REMOTE_NAME_FOR_LOCAL_GIT_REPO),
            )
            .target
            .clone();
        tx.repo_mut().set_local_bookmark_target(name, upstream);
    }
}

/// The visible commit `commit`'s change names now: itself, or what it
/// was rewritten into. None when the change is gone.
fn visible(
    repo: &ReadonlyRepo,
    commit: &Commit,
) -> Result<Option<Commit>, VcsError> {
    let Some(targets) = block_on(repo.resolve_change_id(commit.change_id()))?
    else {
        return Ok(None);
    };
    let ids: Vec<CommitId> = targets
        .visible_with_offsets()
        .map(|(_, id)| id.clone())
        .collect();
    match ids.as_slice() {
        [id] => Ok(Some(repo.store().get_commit(id)?)),
        _ => Ok(None),
    }
}

/// One change on a run's stack, for [`ProjectRepo::stack`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StackChange {
    pub commit_id: String,
    pub change_id: String,
    pub description: String,
    pub conflict: bool,
}

/// The changes `head` has that `base` lacks (all of its ancestors when
/// `None`), oldest first.
fn range(
    repo: &Arc<ReadonlyRepo>,
    head: &CommitId,
    base: Option<&CommitId>,
) -> Result<Vec<StackChange>, VcsError> {
    use futures_util::StreamExt as _;
    use jj_lib::revset::ResolvedRevsetExpression;

    let ids: Vec<CommitId> = {
        let base = match base {
            Some(base) => ResolvedRevsetExpression::commit(base.clone()),
            None => ResolvedRevsetExpression::root(),
        };
        let revset = ResolvedRevsetExpression::commit(head.clone())
            .ancestors()
            .minus(&base.ancestors())
            .evaluate(repo.as_ref())?;
        block_on(revset.stream().collect::<Vec<_>>())
            .into_iter()
            .collect::<Result<_, _>>()?
    };
    ids.iter()
        .rev()
        .map(|id| {
            let commit = repo.store().get_commit(id)?;
            Ok(StackChange {
                commit_id: id.hex(),
                change_id: commit.change_id().reverse_hex(),
                description: commit.description().to_owned(),
                conflict: commit.has_conflict(),
            })
        })
        .collect()
}

/// The commit a full hex id names.
fn commit(repo: &Arc<ReadonlyRepo>, hex: &str) -> Result<Commit, VcsError> {
    let id = CommitId::try_from_hex(hex.trim())
        .ok_or_else(|| VcsError::NotCommitId(hex.to_owned()))?;
    repo.store()
        .get_commit(&id)
        .map_err(|source| VcsError::MissingCommit {
            hex: hex.to_owned(),
            source,
        })
}

/// Makes `into` a bare copy of the Git repository at `source`, without
/// `git`: the object files are hard-linked (copied across file systems),
/// the refs, `HEAD` and config copied, and the config marked bare.
/// Objects never change once written, so sharing them is safe.
fn copy_git_store(source: &Path, into: &Path) -> Result<(), VcsError> {
    let git_dir = git_dir(source)
        .ok_or_else(|| VcsError::NotGitRepo(source.to_owned()))?;
    let copied = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(into)?;
        copy_tree(&git_dir.join("objects"), &into.join("objects"), true)?;
        copy_tree(&git_dir.join("refs"), &into.join("refs"), false)?;
        copy_files(&git_dir, into)?;
        copy_head(&git_dir, into)?;
        let config =
            std::fs::read_to_string(git_dir.join("config")).unwrap_or_default();
        std::fs::write(into.join("config"), bare_config(&config))?;
        Ok(())
    })();
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(into);
    }
    copied.map_err(|error| VcsError::CopyGitStore {
        path: source.to_owned(),
        source: error,
    })
}

/// Brings a copy made by [`copy_git_store`] up to date with its source:
/// the objects it lacks (object files never change, so the ones it has
/// are kept), and the source's branches and tags. A checkout's `HEAD`
/// names the branch checked out in it, not its default one, so the
/// copy keeps the `HEAD` it had from the import; a bare repository's is
/// its default branch, and comes along.
fn update_git_store(source: &Path, into: &Path) -> Result<(), VcsError> {
    let git_dir = git_dir(source)
        .ok_or_else(|| VcsError::NoLongerGitRepo(source.to_owned()))?;
    (|| -> std::io::Result<()> {
        add_missing(&git_dir.join("objects"), &into.join("objects"))?;
        // The source's branches and tags as they are: a loose ref left
        // from before would win over the source's packed one.
        for refs in ["heads", "tags"] {
            let target = into.join("refs").join(refs);
            if target.is_dir() {
                std::fs::remove_dir_all(&target)?;
            }
            copy_tree(&git_dir.join("refs").join(refs), &target, false)?;
        }
        copy_files(&git_dir, into)?;
        if git_dir == source {
            copy_head(&git_dir, into)?;
        }
        Ok(())
    })()
    .map_err(|error| VcsError::UpdateGitStore {
        path: source.to_owned(),
        source: error,
    })
}

/// Copies the files beside the refs that say what the refs are: the
/// packed refs, and `shallow`, the commits of a shallow clone whose
/// parents it lacks. One the source does not have goes from the copy:
/// a stale `shallow` would cut history short, and stale packed refs
/// bring back branches.
fn copy_files(git_dir: &Path, into: &Path) -> std::io::Result<()> {
    for file in ["packed-refs", "shallow"] {
        let from = git_dir.join(file);
        if from.is_file() {
            std::fs::copy(&from, into.join(file))?;
        } else if into.join(file).is_file() {
            std::fs::remove_file(into.join(file))?;
        }
    }
    Ok(())
}

/// Copies the source's `HEAD`: the branch trunk follows.
fn copy_head(git_dir: &Path, into: &Path) -> std::io::Result<()> {
    std::fs::copy(git_dir.join("HEAD"), into.join("HEAD")).map(drop)
}

/// Links (or copies) the files under `from` that `into` lacks.
fn add_missing(from: &Path, into: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(into)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = into.join(entry.file_name());
        if is_dir(&entry)? {
            add_missing(&entry.path(), &target)?;
        } else if !target.exists() {
            link_or_copy(&entry, &target)?;
        }
    }
    Ok(())
}

/// The directory holding a repository's objects and refs: `.git` of a
/// checkout, the common directory of a linked worktree, or the
/// repository itself when it is bare.
fn git_dir(source: &Path) -> Option<PathBuf> {
    let dot_git = source.join(".git");
    let dir = if dot_git.is_dir() {
        dot_git
    } else if dot_git.is_file() {
        // A linked worktree: `gitdir: <path>`, whose `commondir` names
        // the repository the worktree belongs to.
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let pointed =
            PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        let pointed = if pointed.is_absolute() {
            pointed
        } else {
            source.join(pointed)
        };
        match std::fs::read_to_string(pointed.join("commondir")) {
            Ok(common) => pointed.join(common.trim()),
            Err(_) => pointed,
        }
    } else {
        source.to_owned()
    };
    (dir.join("objects").is_dir() && dir.join("HEAD").is_file()).then_some(dir)
}

/// `config` for a bare copy: `bare = true`, and no worktree of its own.
fn bare_config(config: &str) -> String {
    let mut lines: Vec<String> = config
        .lines()
        .filter(|line| {
            let key = line.trim().split('=').next().unwrap_or("").trim();
            !key.eq_ignore_ascii_case("bare")
                && !key.eq_ignore_ascii_case("worktree")
        })
        .map(str::to_owned)
        .collect();
    match lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case("[core]"))
    {
        Some(core) => lines.insert(core + 1, "\tbare = true".to_owned()),
        None => lines
            .splice(0..0, ["[core]".to_owned(), "\tbare = true".to_owned()])
            .for_each(drop),
    }
    lines.join("\n") + "\n"
}

/// Copies the files under `from` to `into`, hard-linking them when
/// `link` is set and the file system allows it.
fn copy_tree(from: &Path, into: &Path, link: bool) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(into)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = into.join(entry.file_name());
        if is_dir(&entry)? {
            copy_tree(&entry.path(), &target, link)?;
        } else if link {
            link_or_copy(&entry, &target)?;
        } else {
            copy(&entry, &target)?;
        }
    }
    Ok(())
}

/// Whether `entry` is a directory; one that went away is not.
fn is_dir(entry: &std::fs::DirEntry) -> std::io::Result<bool> {
    match entry.file_type() {
        Ok(kind) => Ok(kind.is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Hard-links `entry`, an object file, to `target`, or copies it when
/// linking fails. A `target` that exists already is kept: jj may have
/// written the same object meanwhile, and an object's name is its
/// content's hash.
fn link_or_copy(
    entry: &std::fs::DirEntry,
    target: &Path,
) -> std::io::Result<()> {
    if skipped(entry) {
        return Ok(());
    }
    match std::fs::hard_link(entry.path(), target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(())
        }
        Err(_) => match copy(entry, target) {
            Err(_) if target.exists() => Ok(()),
            copied => copied,
        },
    }
}

/// Copies `entry` to `target`. Git may be working in the store as it is
/// copied, as background maintenance does after a commit: its lock
/// files are skipped, and a file that goes away before it is copied was
/// one of git's own temporary files.
fn copy(entry: &std::fs::DirEntry, target: &Path) -> std::io::Result<()> {
    if skipped(entry) {
        return Ok(());
    }
    match std::fs::copy(entry.path(), target) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Git's lock files, which belong to whatever git process holds them.
fn skipped(entry: &std::fs::DirEntry) -> bool {
    entry.file_name().to_string_lossy().ends_with(".lock")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Git's lock files are left out of a copy of its store: they belong
    /// to a git process that may be working in the source.
    #[test]
    fn a_copy_leaves_out_git_lock_files() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        std::fs::create_dir_all(from.join("ab")).unwrap();
        std::fs::write(from.join("ab").join("cdef"), "object").unwrap();
        std::fs::write(from.join("maintenance.lock"), "").unwrap();
        let into = dir.path().join("into");
        copy_tree(&from, &into, true).unwrap();
        assert!(into.join("ab").join("cdef").exists());
        assert!(!into.join("maintenance.lock").exists());
        let again = dir.path().join("again");
        add_missing(&from, &again).unwrap();
        assert!(!again.join("maintenance.lock").exists());
    }

    /// An update links in the objects its copy lacks while jj may write
    /// objects into the same store: a chat's commit can write a file
    /// upstream has too. An object that appears between the check and
    /// the link is the same object, since its name is its content's
    /// hash, and is kept.
    #[test]
    fn an_object_written_meanwhile_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("cdef"), "object").unwrap();
        let into = dir.path().join("into");
        std::fs::create_dir_all(&into).unwrap();
        let target = into.join("cdef");
        std::fs::write(&target, "object").unwrap();
        // Object files are read-only, as git and jj write them.
        let mut mode = std::fs::metadata(&target).unwrap().permissions();
        mode.set_readonly(true);
        std::fs::set_permissions(&target, mode).unwrap();
        let entry = std::fs::read_dir(&from).unwrap().next().unwrap().unwrap();
        link_or_copy(&entry, &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "object");
    }

    #[test]
    fn bare_config_sets_bare_and_drops_the_worktree() {
        let config = "[core]\n\tbare = false\n\tworktree = ../x\n\tfilemode = true\n[remote \"origin\"]\n\turl = u\n";
        assert_eq!(
            bare_config(config),
            "[core]\n\tbare = true\n\tfilemode = true\n[remote \"origin\"]\n\turl = u\n"
        );
        assert_eq!(bare_config(""), "[core]\n\tbare = true\n");
    }
}
