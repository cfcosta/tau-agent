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

use anyhow::{Context as _, anyhow, bail};
use jj_lib::{
    backend::CommitId,
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
    ) -> anyhow::Result<Self> {
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
    ) -> anyhow::Result<Self> {
        let root = root.into();
        if !root.join(MAIN).join(".jj").is_dir() {
            bail!("No tau project at {}", root.display());
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
    ) -> anyhow::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .with_context(|| format!("Cannot create {}", root.display()))?;
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
        .context("Cannot make the jj repository")?;
        let mut tx = repo.start_transaction();
        let options = GitImportOptions {
            abandon_unreachable_commits: true,
            record_synthetic_predecessors: false,
            remote_auto_track_bookmarks: HashMap::new(),
        };
        block_on(import_refs(tx.repo_mut(), &options))
            .context("Cannot import the Git branches")?;
        block_on(tx.commit("tau: import"))?;
        Ok(project)
    }

    fn new(root: PathBuf, identity: Identity) -> anyhow::Result<Self> {
        let settings = settings(&identity)?;
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                identity,
                settings,
            }),
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
    pub fn trunk(&self) -> anyhow::Result<String> {
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

    /// Makes run `name`'s workspace, on a new empty commit on top of
    /// `base` (a full commit id in hex), with `base`'s files checked out,
    /// and opens it. Opens it as it is if it exists already.
    pub fn add_workspace(&self, name: &str, base: &str) -> anyhow::Result<Vcs> {
        let dir = self.workspace_dir(name);
        if dir.join(".jj").is_dir() {
            return Vcs::open(dir, self.inner.identity.clone());
        }
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Cannot create {}", dir.display()))?;
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
            .context("Cannot add the workspace")?;
        let base = commit(&repo, base)?;
        let mut tx = repo.start_transaction();
        let wc = block_on(
            tx.repo_mut().check_out(WorkspaceNameBuf::from(name), &base),
        )?;
        // Checking out abandons the empty commit the workspace began on.
        block_on(tx.repo_mut().rebase_descendants())?;
        let repo = block_on(tx.commit(format!("tau: add workspace {name}")))?;
        block_on(workspace.check_out(repo.op_id().clone(), None, &wc))
            .context("Cannot check out the workspace's files")?;
        Vcs::open(dir, self.inner.identity.clone())
    }

    /// Removes run `name`'s workspace: jj forgets it, and its directory
    /// is deleted. Its commits stay in the repository.
    pub fn forget_workspace(&self, name: &str) -> anyhow::Result<()> {
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
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("Cannot delete {}", dir.display()))?;
        }
        Ok(())
    }

    /// How the files differ from commit `from` to commit `to` (full hex
    /// ids): one entry per changed file, in path order, with its line
    /// counts and its unified diff.
    pub fn diff(&self, from: &str, to: &str) -> anyhow::Result<Vec<FileDiff>> {
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
    pub fn workspaces(&self) -> anyhow::Result<Vec<String>> {
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

    fn main(&self) -> anyhow::Result<Workspace> {
        Workspace::load(
            &self.inner.settings,
            &self.inner.root.join(MAIN),
            &default_backend_factories(),
            &default_working_copy_factories(),
        )
        .with_context(|| {
            format!("No tau project at {}", self.inner.root.display())
        })
    }

    fn load(&self) -> anyhow::Result<Arc<ReadonlyRepo>> {
        let main = self.main()?;
        Ok(block_on(main.repo_loader().load_at_head())?)
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
            let body = text.lines().filter(|line| {
                !line.starts_with("+++") && !line.starts_with("---")
            });
            let (mut added, mut removed) = (0, 0);
            for line in body {
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
fn commit(repo: &Arc<ReadonlyRepo>, hex: &str) -> anyhow::Result<Commit> {
    let id = CommitId::try_from_hex(hex.trim())
        .ok_or_else(|| anyhow!("`{hex}` is not a commit id"))?;
    repo.store()
        .get_commit(&id)
        .with_context(|| format!("No commit {hex}"))
}

/// Makes `into` a bare copy of the Git repository at `source`, without
/// `git`: the object files are hard-linked (copied across file systems),
/// the refs, `HEAD` and config copied, and the config marked bare.
/// Objects never change once written, so sharing them is safe.
fn copy_git_store(source: &Path, into: &Path) -> anyhow::Result<()> {
    let git_dir = git_dir(source).ok_or_else(|| {
        anyhow!(
            "{} is not a Git repository. Only local repositories can be \
             imported for now",
            source.display()
        )
    })?;
    let copied = (|| -> anyhow::Result<()> {
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
    copied.with_context(|| {
        format!("Cannot copy the Git store of {}", source.display())
    })
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
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target, link)?;
        } else if !link || std::fs::hard_link(entry.path(), &target).is_err() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
