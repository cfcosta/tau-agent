//! [`Project`]: a repository tau owns, with a jj workspace per run
//! (`docs/reference/vcs.md`, "Projects").
//!
//! A project lives in a directory of its own, usually under
//! `$XDG_DATA_HOME/tau/repos/`:
//!
//! - `git/`: a bare copy of the source's Git store, which jj writes to;
//! - `main/`: the jj repository, whose own working copy stays empty;
//! - `runs/<name>/`: one jj workspace per run, each on its own commit.
//!
//! Runs never touch the user's own checkout. The functions here block;
//! call them off the async executor.

use std::{
    collections::HashMap,
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
    git::{GitImportOptions, import_refs},
    matchers::EverythingMatcher,
    object_id::ObjectId as _,
    ref_name::{RefName, WorkspaceNameBuf},
    repo::{ReadonlyRepo, Repo as _},
    settings::UserSettings,
    workspace::Workspace,
};
use pollster::block_on;

use crate::{
    diff::{ChangeKind, FileChange},
    error::VcsError,
    run_workspace::Link,
    vcs::{Identity, Vcs, settings},
};

const GIT: &str = "git";
const MAIN: &str = "main";
const RUNS: &str = "runs";

/// A repository tau owns. Cheap to clone.
#[derive(Clone)]
pub struct Project {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    identity: Identity,
    settings: UserSettings,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("root", &self.inner.root)
            .finish()
    }
}

impl Project {
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
    /// them now. Runs keep their workspaces and commits; new runs start
    /// from the new trunk. Returns the trunk before and after.
    pub fn update(&self, from: UpdateFrom<'_>) -> Result<Updated, VcsError> {
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
        let options = GitImportOptions {
            abandon_unreachable_commits: false,
            record_synthetic_predecessors: false,
            remote_auto_track_bookmarks: HashMap::new(),
        };
        block_on(import_refs(tx.repo_mut(), &options))
            .map_err(VcsError::ImportBranches)?;
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
        self.inner.root.join(RUNS).join(name)
    }

    /// The commit new runs start from: the source's default branch, as
    /// the clone's `HEAD` names it, or the root commit of an empty
    /// repository. A full commit id, in hex.
    pub fn trunk(&self) -> Result<String, VcsError> {
        let repo = self.load()?;
        let head =
            std::fs::read_to_string(self.inner.root.join(GIT).join("HEAD"))
                .unwrap_or_default();
        let branch = head
            .trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_owned);
        let view = repo.view();
        let target = branch
            .iter()
            .map(String::as_str)
            .chain(["main", "master", "trunk"])
            .find_map(|name| {
                view.get_local_bookmark(RefName::new(name))
                    .as_normal()
                    .cloned()
            });
        let id =
            target.unwrap_or_else(|| repo.store().root_commit_id().clone());
        Ok(id.hex())
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
        use futures_util::StreamExt as _;
        use jj_lib::revset::ResolvedRevsetExpression;

        let repo = self.load()?;
        let id = |hex: &str| {
            CommitId::try_from_hex(hex)
                .ok_or_else(|| VcsError::NotCommitId(hex.to_owned()))
        };
        let ids: Vec<CommitId> = {
            let revset = ResolvedRevsetExpression::commit(id(head)?)
                .ancestors()
                .minus(&ResolvedRevsetExpression::commit(id(keep)?).ancestors())
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
    /// and opens it. Opens it as it is if it exists already.
    pub fn add_workspace(
        &self,
        name: &str,
        base: &str,
    ) -> Result<Vcs, VcsError> {
        let dir = self.workspace_dir(name);
        if dir.join(".jj").is_dir() {
            return Vcs::open(dir, self.inner.identity.clone());
        }
        std::fs::create_dir_all(&dir).map_err(|source| VcsError::Create {
            path: dir.clone(),
            source,
        })?;
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
        let base = commit(&repo, base)?;
        let mut tx = repo.start_transaction();
        let wc = block_on(
            tx.repo_mut().check_out(WorkspaceNameBuf::from(name), &base),
        )?;
        // Checking out abandons the empty commit the workspace began on.
        block_on(tx.repo_mut().rebase_descendants())?;
        let repo = block_on(tx.commit(format!("tau: add workspace {name}")))?;
        block_on(workspace.check_out(repo.op_id().clone(), None, &wc))
            .map_err(VcsError::CheckOut)?;
        Vcs::open(dir, self.inner.identity.clone())
    }

    /// Removes run `name`'s workspace: jj forgets it, and its directory
    /// is deleted. Its commits stay in the repository.
    pub fn forget_workspace(&self, name: &str) -> Result<(), VcsError> {
        let repo = self.load()?;
        let name_buf = WorkspaceNameBuf::from(name);
        if repo.view().get_wc_commit_id(&name_buf).is_some() {
            let mut tx = repo.start_transaction();
            block_on(tx.repo_mut().remove_workspace(&name_buf))?;
            block_on(tx.repo_mut().rebase_descendants())?;
            block_on(tx.commit(format!("tau: forget workspace {name}")))?;
        }
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

    fn load(&self) -> Result<Arc<ReadonlyRepo>, VcsError> {
        let main = self.main()?;
        Ok(block_on(main.repo_loader().load_at_head())?)
    }
}

/// Where [`Project::update`] brings changes from.
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
        for file in ["packed-refs", "HEAD"] {
            let from = git_dir.join(file);
            if from.is_file() {
                std::fs::copy(&from, into.join(file))?;
            }
        }
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
/// are kept), and the source's branches, tags and `HEAD`.
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
        for file in ["packed-refs", "HEAD"] {
            let from = git_dir.join(file);
            if from.is_file() {
                std::fs::copy(&from, into.join(file))?;
            }
        }
        Ok(())
    })()
    .map_err(|error| VcsError::UpdateGitStore {
        path: source.to_owned(),
        source: error,
    })
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

/// Hard-links `entry` to `target`, or copies it when linking fails.
fn link_or_copy(
    entry: &std::fs::DirEntry,
    target: &Path,
) -> std::io::Result<()> {
    if skipped(entry) || std::fs::hard_link(entry.path(), target).is_ok() {
        return Ok(());
    }
    copy(entry, target)
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
