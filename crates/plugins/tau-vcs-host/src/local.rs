//! Looking at a folder on this computer before tau adds it as a local
//! repository (ADR 0043, `docs/reference/vcs.md`, "Inspecting a
//! folder"): what tau found there, or why it refuses the folder and
//! what to add instead. Setting jj up in it, once found, is
//! `ProjectRepo::setup_local`'s (`setup`).
//!
//! Inspecting only reads, with gix, and with jj-lib for a jj
//! repository: nothing in the folder changes, not even Git's index,
//! which `git status` refreshes. Git's config is the repository's own,
//! with the person's excludes file from the [`JjEnv`]; the rest of the
//! person's Git config is not read.
//!
//! Blocks: [`inspect`] runs it on tokio's blocking pool (ADR 0028).

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};

use futures_util::StreamExt as _;
use gix::{
    bstr::{BStr, ByteSlice as _},
    prelude::ObjectIdExt as _,
};
use jj_lib::{
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
    },
    git::{REMOTE_NAME_FOR_LOCAL_GIT_REPO, get_git_backend},
    git_backend::GitRepoAtWorkdirError,
    repo::Repo as _,
    workspace::{
        DefaultWorkspaceLoaderFactory,
        Workspace,
        WorkspaceLoaderFactory as _,
    },
};
use pollster::block_on;

use crate::{Identity, config::JjEnv, revsets::ConfigRevsets};

mod setup;

pub use setup::{State, Step, runs_dir_name, unfinished};
pub(crate) use setup::{is_local, setup};

/// Why a read failed, for [`Refusal::Unreadable`].
type Failed = Box<dyn std::error::Error + Send + Sync>;

/// What jj's hint says to run in a jj repository whose Git store is
/// inside `.jj`, so that tau can add it.
pub const COLOCATE_COMMAND: &str = "jj git colocation enable";

/// What kind of repository a folder is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Git repository without jj.
    Git,
    /// A jj repository colocated with Git: `.jj` beside `.git`.
    Jj,
}

/// What tau found in a folder it can add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub kind: Kind,
    /// The repository's folder, with symbolic links resolved: what `git
    /// rev-parse --show-toplevel` prints there.
    pub root: PathBuf,
    /// The checked-out branch, also when it has no commits yet; none when
    /// `HEAD` is detached.
    pub branch: Option<String>,
    /// The `origin` remote, when the repository has one.
    pub origin: Option<Origin>,
    /// How many commits `HEAD` reaches: 0 before the first commit.
    pub commits: u64,
    /// The paths with uncommitted changes, sorted, as `git status
    /// --porcelain` lists them with renames off: changed and staged
    /// files, and untracked ones, an untracked folder as one path ending
    /// in `/`. Ignored files are left out.
    pub uncommitted: Vec<String>,
    /// The branch tau would take as trunk, when there is one.
    pub trunk: Option<Trunk>,
}

/// A repository's `origin` remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// Its URL, as `remote.origin.url` holds it.
    pub url: String,
    /// The GitHub repository it names, when it is on GitHub.
    pub github: Option<GitHub>,
}

/// A repository on GitHub: `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHub {
    pub owner: String,
    pub name: String,
}

/// The branch tau would take as trunk, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trunk {
    pub branch: String,
    pub from: TrunkFrom,
}

/// Where a [`Trunk`] came from, first match winning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrunkFrom {
    /// The branch `refs/remotes/origin/HEAD` names, when it resolves.
    OriginHead,
    /// jj's `trunk()`, in a jj repository, when it names a commit other
    /// than the root: the first bookmark there by name, local or on a
    /// remote other than Git.
    JjTrunk,
    /// The checked-out branch.
    CheckedOut,
}

/// Why tau does not add a folder, each saying what to do instead. The
/// checks run in this order, the first that holds winning.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("{} does not exist", .0.display())]
    Missing(PathBuf),
    #[error("{} is a file, not a folder", .0.display())]
    NotAFolder(PathBuf),
    #[error("{} could not be read: {reason}", path.display())]
    Unreadable { path: PathBuf, reason: String },
    #[error("{} is not a Git or jj repository", .0.display())]
    NotARepository(PathBuf),
    #[error(
        "{} is a bare repository: it has no checkout to work in",
        .0.display()
    )]
    Bare(PathBuf),
    #[error("{}", worktree_text(main.as_deref()))]
    Worktree {
        /// The repository's main checkout, none when the repository is
        /// bare.
        main: Option<PathBuf>,
    },
    #[error(
        "This folder is inside the repository at {}: add that folder instead",
        root.display()
    )]
    Inside { root: PathBuf },
    #[error(
        "{} is a jj repository whose Git store is inside .jj. Run `{hint}` \
         there, then add it again",
        root.display()
    )]
    JjNotColocated { root: PathBuf, hint: &'static str },
    #[error(
        "The repository uses Git LFS ({found_in}): runs would see pointer \
         files instead"
    )]
    Lfs {
        /// Where `filter=lfs` or the LFS config was found.
        found_in: String,
    },
    #[error(
        "The repository uses submodules ({}): runs would see empty folders",
        submodules_text(paths)
    )]
    Submodules {
        /// The submodules' paths, from the gitlinks in `HEAD` and the
        /// index and from `.gitmodules`; empty when `.gitmodules` names
        /// none.
        paths: Vec<String>,
    },
}

fn worktree_text(main: Option<&Path>) -> String {
    match main {
        Some(main) => format!(
            "This folder is a Git worktree of {}: add the main checkout \
             instead",
            main.display()
        ),
        None => "This folder is a Git worktree of a bare repository, which \
                 has no main checkout to add"
            .to_owned(),
    }
}

fn submodules_text(paths: &[String]) -> String {
    if paths.is_empty() {
        ".gitmodules".to_owned()
    } else {
        paths.join(", ")
    }
}

/// What tau finds in the folder `path`, or why it refuses it. See the
/// module's documentation.
pub async fn inspect(
    path: impl Into<PathBuf>,
    identity: Identity,
) -> Result<Found, Refusal> {
    let path = path.into();
    tokio::task::spawn_blocking(move || inspect_blocking(&path, &identity))
        .await
        .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()))
}

/// [`inspect`], on this thread. Blocks.
pub fn inspect_blocking(
    path: &Path,
    identity: &Identity,
) -> Result<Found, Refusal> {
    let unreadable = |error: &dyn std::fmt::Display| Refusal::Unreadable {
        path: path.to_owned(),
        reason: error.to_string(),
    };
    match std::fs::metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Refusal::Missing(path.to_owned()));
        }
        Err(error) => return Err(unreadable(&error)),
        Ok(metadata) if !metadata.is_dir() => {
            return Err(Refusal::NotAFolder(path.to_owned()));
        }
        Ok(_) => {}
    }
    std::fs::read_dir(path).map_err(|error| unreadable(&error))?;
    let folder =
        std::fs::canonicalize(path).map_err(|error| unreadable(&error))?;
    // jj finds its workspace as the nearest folder with a `.jj` in it.
    // A `.jj` a setup made and did not finish is tau's, which the next
    // setup takes away: the folder is still the Git repository it was.
    let jj_root = folder
        .ancestors()
        .find(|dir| dir.join(".jj").is_dir() && !unfinished(dir))
        .map(Path::to_owned);
    let repo = discover(&folder)?;
    // The nearer of the two is the repository the folder is in.
    let git_top = repo.as_ref().map(|repo| {
        repo.workdir()
            .map(|dir| {
                std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned())
            })
            .unwrap_or_else(|| repo.git_dir().to_owned())
    });
    if let Some(jj_root) = &jj_root
        && git_top
            .as_ref()
            .is_none_or(|top| jj_root != top && jj_root.starts_with(top))
    {
        return Err(jj_only(&folder, jj_root));
    }
    let Some(repo) = repo else {
        return Err(Refusal::NotARepository(folder));
    };
    let Some(root) =
        git_top.filter(|_| !repo.is_bare() && repo.workdir().is_some())
    else {
        return Err(Refusal::Bare(repo.git_dir().to_owned()));
    };
    if repo.kind() == gix::repository::Kind::LinkedWorkTree {
        return Err(Refusal::Worktree {
            main: main_checkout(repo.common_dir()),
        });
    }
    if folder != root {
        return Err(Refusal::Inside { root });
    }
    let workspace = match &jj_root {
        Some(jj_root) if *jj_root == root => {
            Some(colocated(identity, &root)?.ok_or_else(|| {
                Refusal::JjNotColocated {
                    root: root.clone(),
                    hint: COLOCATE_COMMAND,
                }
            })?)
        }
        _ => None,
    };
    let unreadable = |error: &dyn std::fmt::Display| Refusal::Unreadable {
        path: root.clone(),
        reason: error.to_string(),
    };
    let head = Head::read(&repo).map_err(|error| unreadable(&error))?;
    if let Some(found_in) =
        lfs(&repo, &root, &head).map_err(|error| unreadable(&error))?
    {
        return Err(Refusal::Lfs { found_in });
    }
    if let Some(paths) =
        submodules(&repo, &root, &head).map_err(|error| unreadable(&error))?
    {
        return Err(Refusal::Submodules { paths });
    }
    let branch = repo
        .head_name()
        .map_err(|error| unreadable(&error))?
        .and_then(|name| {
            let name = name.as_bstr().strip_prefix(b"refs/heads/")?;
            Some(name.to_str_lossy().into_owned())
        });
    let trunk = match origin_head(&repo).map_err(|error| unreadable(&error))? {
        Some(branch) => Some(Trunk {
            branch,
            from: TrunkFrom::OriginHead,
        }),
        None => {
            let jj = match &workspace {
                Some(workspace) => {
                    jj_trunk(workspace).map_err(|error| unreadable(&error))?
                }
                None => None,
            };
            jj.map(|branch| Trunk {
                branch,
                from: TrunkFrom::JjTrunk,
            })
            .or_else(|| {
                branch.clone().map(|branch| Trunk {
                    branch,
                    from: TrunkFrom::CheckedOut,
                })
            })
        }
    };
    let origin =
        repo.config_snapshot()
            .string("remote.origin.url")
            .map(|url| {
                let url = url.to_str_lossy().into_owned();
                Origin {
                    github: github(&url),
                    url,
                }
            });
    let commits = match head.commit {
        Some(commit) => commit
            .attach(&repo)
            .ancestors()
            .all()
            .map_err(|error| unreadable(&error))?
            .try_fold(0u64, |count, info| info.map(|_| count + 1))
            .map_err(|error| unreadable(&error))?,
        None => 0,
    };
    let uncommitted = uncommitted(&repo, &identity.env, &root)
        .map_err(|error| unreadable(&error))?;
    Ok(Found {
        kind: if workspace.is_some() {
            Kind::Jj
        } else {
            Kind::Git
        },
        root,
        branch,
        origin,
        commits,
        uncommitted,
        trunk,
    })
}

/// The Git repository `folder` is in, opened with its own config alone,
/// or none when it is in none.
fn discover(folder: &Path) -> Result<Option<gix::Repository>, Refusal> {
    let options = gix::open::Options::isolated();
    let mapping = gix::sec::trust::Mapping {
        full: options.clone(),
        reduced: options,
    };
    match gix::ThreadSafeRepository::discover_opts(
        folder,
        gix::discover::upwards::Options::default(),
        mapping,
    ) {
        Ok(repo) => Ok(Some(repo.to_thread_local())),
        Err(error) if error.is_not_found() => Ok(None),
        Err(error) => Err(Refusal::Unreadable {
            path: folder.to_owned(),
            reason: error.to_string(),
        }),
    }
}

/// Why tau refuses `folder` in the jj workspace at `jj_root`, which no
/// Git checkout of the same folder holds.
fn jj_only(folder: &Path, jj_root: &Path) -> Refusal {
    // Another workspace of a repository has a `.jj/repo` file naming the
    // `.jj/repo` of the repository's own.
    let repo_file = jj_root.join(".jj").join("repo");
    if repo_file.is_file()
        && let Ok(text) = std::fs::read_to_string(&repo_file)
    {
        let repo_dir = jj_root.join(".jj").join(text.trim());
        let main = std::fs::canonicalize(&repo_dir)
            .ok()
            .and_then(|dir| Some(dir.parent()?.parent()?.to_owned()));
        return Refusal::Worktree { main };
    }
    if folder != jj_root {
        return Refusal::Inside {
            root: jj_root.to_owned(),
        };
    }
    Refusal::JjNotColocated {
        root: jj_root.to_owned(),
        hint: COLOCATE_COMMAND,
    }
}

/// The main checkout of the repository whose common Git directory is
/// `common_dir`: the folder its `.git` is in, none when it is bare.
fn main_checkout(common_dir: &Path) -> Option<PathBuf> {
    let common_dir = std::fs::canonicalize(common_dir)
        .unwrap_or_else(|_| common_dir.to_owned());
    if common_dir.file_name()? != ".git" {
        return None;
    }
    Some(common_dir.parent()?.to_owned())
}

/// The jj workspace at `root`, when its Git store is the one checked out
/// at `root`, as jj-cli's `is_colocated_git_workspace` finds it.
fn colocated(
    identity: &Identity,
    root: &Path,
) -> Result<Option<Workspace>, Refusal> {
    let unreadable = |error: &dyn std::fmt::Display| Refusal::Unreadable {
        path: root.to_owned(),
        reason: error.to_string(),
    };
    let loader = DefaultWorkspaceLoaderFactory
        .create(root)
        .map_err(|error| unreadable(&error))?;
    let settings = crate::config::settings(
        identity,
        loader.workspace_root(),
        loader.repo_path(),
    )
    .map_err(|error| unreadable(&error))?;
    let workspace = loader
        .load(
            &settings,
            &default_backend_factories(),
            &default_working_copy_factories(),
        )
        .map_err(|error| unreadable(&error))?;
    let Ok(backend) = get_git_backend(workspace.repo_loader().store()) else {
        return Ok(None);
    };
    match backend.open_git_repo_at_workdir(root) {
        Ok(_) => Ok(Some(workspace)),
        Err(
            GitRepoAtWorkdirError::NotFound { .. }
            | GitRepoAtWorkdirError::Unrelated { .. },
        ) => Ok(None),
        Err(error) => Err(unreadable(&error)),
    }
}

/// The first bookmark by name at the commit jj's `trunk()` names in
/// `workspace`, local or on a remote other than Git; none when `trunk()`
/// is the root commit, names a commit no bookmark is at, or the
/// operation log has several heads (merging them would write).
fn jj_trunk(workspace: &Workspace) -> Result<Option<String>, Failed> {
    let loader = workspace.repo_loader();
    let heads = block_on(loader.op_heads_store().get_op_heads())?;
    let [head] = heads.as_slice() else {
        return Ok(None);
    };
    let operation = block_on(loader.load_operation(head))?;
    let repo = block_on(loader.load_at(&operation))?;
    let revsets = ConfigRevsets::new(
        workspace.settings(),
        workspace.workspace_root(),
        workspace.workspace_name(),
    )?;
    let expression = revsets.parse(repo.as_ref(), "trunk()", "trunk()")?;
    let revset = revsets.evaluate(repo.as_ref(), "trunk()", &expression)?;
    let first = block_on(revset.stream().next()).transpose()?;
    let Some(commit) = first.filter(|id| id != repo.store().root_commit_id())
    else {
        return Ok(None);
    };
    let view = repo.view();
    let local = view
        .local_bookmarks_for_commit(&commit)
        .map(|(name, _)| name.as_str().to_owned());
    let remote = view
        .all_remote_bookmarks()
        .filter(|(symbol, remote_ref)| {
            symbol.remote != REMOTE_NAME_FOR_LOCAL_GIT_REPO
                && remote_ref.target.present_adds().any(|id| *id == commit)
        })
        .map(|(symbol, _)| symbol.name.as_str().to_owned());
    Ok(local.chain(remote).min())
}

/// The branch `refs/remotes/origin/HEAD` names, when it names one that
/// exists.
fn origin_head(repo: &gix::Repository) -> Result<Option<String>, gix::Error> {
    let Some(head) = repo.try_find_reference("refs/remotes/origin/HEAD")?
    else {
        return Ok(None);
    };
    let gix::refs::TargetRef::Symbolic(target) = head.target() else {
        return Ok(None);
    };
    let Some(branch) = target.as_bstr().strip_prefix(b"refs/remotes/origin/")
    else {
        return Ok(None);
    };
    if repo.try_find_reference(target.as_bstr())?.is_none() {
        return Ok(None);
    }
    Ok(Some(branch.to_str_lossy().into_owned()))
}

/// The GitHub repository `url` names: an HTTPS, HTTP, `git://` or SSH
/// URL, or Git's `git@github.com:owner/name` form, on `github.com`
/// (`www.` and `ssh.` too), with or without `.git` and a closing `/`.
pub fn github(url: &str) -> Option<GitHub> {
    let parsed = gix::url::parse(BStr::new(url)).ok()?;
    use gix::url::Scheme;
    if !matches!(
        parsed.scheme,
        Scheme::Https | Scheme::Http | Scheme::Ssh | Scheme::Git
    ) {
        return None;
    }
    let host = parsed.host()?.to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "github.com" | "www.github.com" | "ssh.github.com"
    ) {
        return None;
    }
    let path = parsed.path.to_str().ok()?;
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let owner_ok = !owner.is_empty()
        && owner.len() <= 39
        && !owner.starts_with('-')
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let name_ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (owner_ok && name_ok).then(|| GitHub {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

/// What `HEAD` holds: its commit, none before the first, and its tree's
/// files the checks read.
struct Head {
    commit: Option<gix::ObjectId>,
    /// Every entry of the tree, with its path from the root.
    entries: Vec<gix::traverse::tree::recorder::Entry>,
}

impl Head {
    fn read(repo: &gix::Repository) -> Result<Self, gix::Error> {
        let mut head = repo.head()?;
        if head.is_unborn() {
            return Ok(Self {
                commit: None,
                entries: Vec::new(),
            });
        }
        let commit = head.peel_to_commit()?;
        let mut recorder = gix::traverse::tree::Recorder::default();
        commit.tree()?.traverse().breadthfirst(&mut recorder)?;
        Ok(Self {
            commit: Some(commit.id),
            entries: recorder.records,
        })
    }

    /// The blob at `path` from the root, when there is one.
    fn blob(&self, path: &str) -> Option<gix::ObjectId> {
        self.entries
            .iter()
            .find(|entry| entry.mode.is_blob() && entry.filepath == path)
            .map(|entry| entry.oid)
    }
}

/// Whether `path` (from the root) names a `.gitattributes` file.
fn is_attributes(path: &BStr) -> bool {
    path == ".gitattributes" || path.ends_with(b"/.gitattributes")
}

/// Where the repository asks for Git LFS, when it does: `filter=lfs` in
/// a `.gitattributes` of `HEAD` or the index, in the working tree's root
/// `.gitattributes` or in `info/attributes`, or a `.lfsconfig` in the
/// working tree or `HEAD`.
fn lfs(
    repo: &gix::Repository,
    root: &Path,
    head: &Head,
) -> Result<Option<String>, gix::Error> {
    let blob_says = |id: gix::ObjectId| -> Result<bool, gix::Error> {
        Ok(asks_for_lfs(&repo.find_blob(id)?.data))
    };
    for entry in &head.entries {
        if entry.mode.is_blob()
            && is_attributes(entry.filepath.as_ref())
            && blob_says(entry.oid)?
        {
            return Ok(Some(format!("{} at HEAD", entry.filepath)));
        }
    }
    let index = repo.index_or_empty()?;
    for entry in index.entries() {
        let path = entry.path(&index);
        if is_attributes(path) && blob_says(entry.id)? {
            return Ok(Some(format!("{path} in the index")));
        }
    }
    let files = [
        (root.join(".gitattributes"), ".gitattributes"),
        (
            repo.common_dir().join("info").join("attributes"),
            "info/attributes",
        ),
    ];
    for (path, name) in files {
        if std::fs::read(&path).is_ok_and(|text| asks_for_lfs(&text)) {
            return Ok(Some(name.to_owned()));
        }
    }
    if root.join(".lfsconfig").exists() || head.blob(".lfsconfig").is_some() {
        return Ok(Some(".lfsconfig".to_owned()));
    }
    Ok(None)
}

/// Whether the attributes `text` set `filter=lfs` anywhere.
fn asks_for_lfs(text: &[u8]) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with(b"#")
            && line
                .fields_with(|c| c.is_whitespace())
                .any(|field| field == b"filter=lfs")
    })
}

/// The repository's submodules' paths, sorted, when it has submodules:
/// gitlinks in `HEAD` or the index, or a `.gitmodules` in the working
/// tree or `HEAD`.
fn submodules(
    repo: &gix::Repository,
    root: &Path,
    head: &Head,
) -> Result<Option<Vec<String>>, gix::Error> {
    let mut paths = BTreeSet::new();
    for entry in &head.entries {
        if entry.mode.is_commit() {
            paths.insert(entry.filepath.to_str_lossy().into_owned());
        }
    }
    let index = repo.index_or_empty()?;
    for entry in index.entries() {
        if entry.mode == gix::index::entry::Mode::COMMIT {
            paths.insert(entry.path(&index).to_str_lossy().into_owned());
        }
    }
    let modules = match std::fs::read(root.join(".gitmodules")) {
        Ok(text) => Some(text),
        Err(_) => match head.blob(".gitmodules") {
            Some(id) => Some(repo.find_blob(id)?.data.clone()),
            None => None,
        },
    };
    if let Some(text) = &modules {
        paths.extend(gitmodules_paths(text));
    }
    if paths.is_empty() && modules.is_none() {
        return Ok(None);
    }
    Ok(Some(paths.into_iter().collect()))
}

/// The `submodule.<name>.path` values of the `.gitmodules` `text`; none
/// when it does not parse.
fn gitmodules_paths(text: &[u8]) -> Vec<String> {
    let Ok(file) = gix::config::File::from_bytes_no_includes(
        text,
        gix::config::file::Metadata::api(),
        Default::default(),
    ) else {
        return Vec::new();
    };
    let Some(sections) = file.sections_by_name("submodule") else {
        return Vec::new();
    };
    sections
        .filter_map(|section| section.value("path"))
        .map(|path| path.to_str_lossy().into_owned())
        .collect()
}

/// The paths `git status --porcelain` lists in the checkout at `root`,
/// renames off, read without writing the index. The person's excludes
/// file comes from `env`, as jj finds it.
///
/// gix's own collapsing of untracked folders takes a folder whose
/// tracked files are all deleted for untracked, which git does not, so
/// the untracked files are listed one by one and collapsed here as git
/// collapses them.
fn uncommitted(
    repo: &gix::Repository,
    env: &JjEnv,
    root: &Path,
) -> Result<Vec<String>, Failed> {
    let mut repo = repo.clone();
    if let Some(excludes) =
        crate::snapshot_config::excludes_file_of(env, repo.git_dir(), root)?
    {
        let mut config = repo.config_snapshot_mut();
        config.set_raw_value(
            "core.excludesFile",
            gix::path::into_bstr(excludes).as_ref(),
        )?;
        config.commit()?;
    }
    let items = repo
        .status(gix::progress::Discard)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .index_worktree_rewrites(None)
        .tree_index_track_renames(
            gix::status::tree_index::TrackRenames::Disabled,
        )
        .index_worktree_submodules(gix::status::Submodule::Given {
            ignore: gix::submodule::config::Ignore::All,
            check_dirty: false,
        })
        .into_iter(Vec::new())?;
    let index = repo.index_or_empty()?;
    let mut paths = BTreeSet::new();
    for item in items {
        match item? {
            gix::status::Item::TreeIndex(change) => {
                paths.insert(change.location().to_str_lossy().into_owned());
            }
            gix::status::Item::IndexWorktree(item) => {
                use gix::status::index_worktree::Item;
                if item.summary().is_none() {
                    continue;
                }
                match &item {
                    Item::DirectoryContents { entry, .. } => {
                        // A repository inside, untracked, is a folder.
                        let folder = matches!(
                            entry.disk_kind,
                            Some(
                                gix::dir::entry::Kind::Directory
                                    | gix::dir::entry::Kind::Repository
                            )
                        );
                        paths.insert(collapsed(
                            &index,
                            entry.rela_path.as_ref(),
                            folder,
                        ));
                    }
                    Item::Rewrite { source, .. } => {
                        paths.insert(
                            source.rela_path().to_str_lossy().into_owned(),
                        );
                        paths.insert(
                            item.rela_path().to_str_lossy().into_owned(),
                        );
                    }
                    Item::Modification { rela_path, .. } => {
                        paths.insert(rela_path.to_str_lossy().into_owned());
                    }
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}

/// How `git status` lists the untracked `path`: as its outermost folder
/// that holds nothing in `index`, ending in `/`, else as itself (with a
/// `/` when it is a `folder`).
fn collapsed(index: &gix::index::State, path: &BStr, folder: bool) -> String {
    let untracked = |dir: &BStr| {
        index
            .prefixed_entries(dir)
            .is_none_or(|entries| entries.is_empty())
    };
    let outermost = path
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'/')
        .map(|(at, _)| &path[..=at])
        .find(|dir| untracked(dir.as_bstr()));
    match outermost {
        Some(dir) => dir.to_str_lossy().into_owned(),
        None if folder => format!("{}/", path.to_str_lossy()),
        None => path.to_str_lossy().into_owned(),
    }
}
