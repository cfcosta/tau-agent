//! Run workspaces as Git worktrees of the project's Git store (ADR
//! 0043, decision 11), so `git`, nix flakes and build scripts that ask
//! Git work in a run's directory.
//!
//! jj-lib 0.46.0's `git::create_worktree` and `unlink_worktree` run
//! `git worktree add --orphan` and `git worktree prune`; tau has no
//! `git`, so it writes the same files itself, as Git's
//! `gitrepository-layout` describes them:
//!
//! - `<common>/worktrees/<id>/gitdir`: the absolute path of the
//!   workspace's `.git`;
//! - `<common>/worktrees/<id>/commondir`: `../..`, the Git store;
//! - `<common>/worktrees/<id>/HEAD`: `ref: refs/jj/root`, jj's unborn
//!   placeholder, until the first operation in the workspace points it
//!   at `@-` (`crate::colocate`);
//! - `<workspace>/.git`: `gitdir: <common>/worktrees/<id>`, absolute.
//!
//! The paths are absolute, as Git writes them by default. jj writes
//! relative ones with `worktree.useRelativePaths`, which makes Git turn
//! on `extensions.relativeWorktrees` in the repository's config; tau
//! never changes the repository's config. The index is left to the
//! first `HEAD` reset.
//!
//! [`link`] makes a workspace's worktree, or finishes one a crash or the
//! person's `git worktree prune` cut short, and [`unlink`] removes one
//! workspace's, never another's: unlike `git worktree prune`, it leaves
//! alone a worktree whose folder is gone, unless it was that
//! workspace's.

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::error::VcsError;

/// The folder under the Git store that holds the worktrees' own files.
const WORKTREES: &str = "worktrees";
/// The longest worktree id [`id_for`] makes.
const MAX_ID: usize = 64;
/// jj-lib's `UNBORN_ROOT_REF_NAME`: what `HEAD` names while `@-` is the
/// root commit, which Git cannot point at.
const UNBORN: &str = "refs/jj/root";

/// The id of the worktree for the workspace named `name`, as `git
/// worktree add` derives one from a folder's name: letters, digits, `-`
/// and `_`, each other character made `-`. A long name (a sub-agent's,
/// `<caller>-sub-<hex>`, of a sub-agent's) keeps its last
/// [`MAX_ID`] characters, where the names differ.
pub(crate) fn id_for(name: &str) -> String {
    let id: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let id = &id[id.len().saturating_sub(MAX_ID)..];
    let id = id.trim_start_matches('-');
    if id.is_empty() {
        "run".to_owned()
    } else {
        id.to_owned()
    }
}

/// Whether the workspace at `workspace` is a whole worktree of the Git
/// store at `common`: its `.git` names a folder of the store's
/// worktrees, whose `gitdir` names that `.git` back, and which has its
/// `commondir` and `HEAD`.
pub(crate) fn is_linked(common: &Path, workspace: &Path) -> bool {
    let Some(admin) = gitlink(workspace) else {
        return false;
    };
    under_worktrees(common, &admin)
        && names(&admin, workspace)
        && admin.join("commondir").is_file()
        && admin.join("HEAD").is_file()
}

/// Makes the workspace `name` at `workspace` (a folder that exists) a
/// worktree of the Git store at `common`, or finishes the one it has:
/// the files a crash left out are written, and a folder of the store's
/// worktrees that names the workspace is used again. Its `HEAD` becomes
/// `head` (a commit id in hex), or jj's unborn placeholder for none.
/// Does nothing when the worktree is whole ([`is_linked`]) or the
/// workspace has a Git directory, and says whether it wrote anything.
pub(crate) fn link(
    common: &Path,
    workspace: &Path,
    name: &str,
    head: Option<&str>,
) -> Result<bool, VcsError> {
    // A Git directory of its own is not tau's to replace.
    if is_linked(common, workspace) || workspace.join(".git").is_dir() {
        return Ok(false);
    }
    let dot_git = canonical(workspace).join(".git");
    let admin = match admin_of(common, workspace)? {
        Some(admin) => admin,
        None => make_admin(common, name)?,
    };
    let write = |file: &str, text: String| -> Result<(), VcsError> {
        let path = admin.join(file);
        std::fs::write(&path, text)
            .map_err(|source| VcsError::Create { path, source })
    };
    // `gitdir` first, so that whatever a crash leaves names its
    // workspace, and is found again.
    write("gitdir", format!("{}\n", dot_git.display()))?;
    write("commondir", "../..\n".to_owned())?;
    write(
        "HEAD",
        match head {
            Some(hex) => format!("{hex}\n"),
            None => format!("ref: {UNBORN}\n"),
        },
    )?;
    std::fs::write(&dot_git, format!("gitdir: {}\n", admin.display()))
        .map_err(|source| VcsError::Create {
            path: dot_git.clone(),
            source,
        })?;
    Ok(true)
}

/// Takes the workspace at `workspace` out of the Git store at `common`:
/// removes every folder of the store's worktrees that names it, and its
/// `.git` when that names one of them. Another worktree is left as it
/// is, even one whose folder is gone. Its files stay.
pub(crate) fn unlink(common: &Path, workspace: &Path) -> Result<(), VcsError> {
    let mut admins = owned(common, workspace)?;
    let linked = gitlink(workspace)
        .filter(|admin| under_worktrees(common, admin) && admin.is_dir());
    if let Some(admin) = &linked
        && !admins.iter().any(|known| same(known, admin))
        && !admin.join("gitdir").exists()
    {
        // A worktree whose `gitdir` a crash left out is this one's.
        admins.push(admin.clone());
    }
    for admin in admins {
        std::fs::remove_dir_all(&admin).or_else(not_found).map_err(
            |source| VcsError::Delete {
                path: admin.clone(),
                source,
            },
        )?;
    }
    let dot_git = workspace.join(".git");
    if dot_git.is_file() && gitlink(workspace).is_some() {
        std::fs::remove_file(&dot_git).or_else(not_found).map_err(
            |source| VcsError::Delete {
                path: dot_git.clone(),
                source,
            },
        )?;
    }
    Ok(())
}

/// The worktree folder to finish for `workspace`: the one its `.git`
/// names, when that is in the store's worktrees and names no other
/// workspace, else one whose `gitdir` names this workspace.
fn admin_of(
    common: &Path,
    workspace: &Path,
) -> Result<Option<PathBuf>, VcsError> {
    if let Some(admin) = gitlink(workspace)
        && under_worktrees(common, &admin)
        && admin.is_dir()
        && (!admin.join("gitdir").exists() || names(&admin, workspace))
    {
        return Ok(Some(admin));
    }
    Ok(owned(common, workspace)?.into_iter().next())
}

/// A new folder in the store's worktrees for workspace `name`: at
/// [`id_for`]'s id, or that id and `-2`, `-3` and so on when it is
/// taken. An empty one is free: what a make cut off right after it
/// made the folder leaves.
fn make_admin(common: &Path, name: &str) -> Result<PathBuf, VcsError> {
    let worktrees = canonical(common).join(WORKTREES);
    std::fs::create_dir_all(&worktrees).map_err(|source| VcsError::Create {
        path: worktrees.clone(),
        source,
    })?;
    let id = id_for(name);
    for n in 1_u32.. {
        let admin = if n == 1 {
            worktrees.join(&id)
        } else {
            worktrees.join(format!("{id}-{n}"))
        };
        match std::fs::create_dir(&admin) {
            Ok(()) => return Ok(admin),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if is_empty(&admin) {
                    return Ok(admin);
                }
            }
            Err(source) => {
                return Err(VcsError::Create {
                    path: admin,
                    source,
                });
            }
        }
    }
    unreachable!("some id is free")
}

/// The folders of the store's worktrees whose `gitdir` names
/// `workspace`'s `.git`, whether or not the workspace exists.
fn owned(common: &Path, workspace: &Path) -> Result<Vec<PathBuf>, VcsError> {
    let worktrees = common.join(WORKTREES);
    let entries = match std::fs::read_dir(&worktrees) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(VcsError::Create {
                path: worktrees,
                source,
            });
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| VcsError::Create {
            path: worktrees.clone(),
            source,
        })?;
        if names(&entry.path(), workspace) {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// The folder `workspace`'s `.git` file names, absolute; none when it
/// has no such file.
fn gitlink(workspace: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(workspace.join(".git")).ok()?;
    let target = text.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    Some(workspace.join(target))
}

/// Whether the worktree folder `admin`'s `gitdir` names `workspace`'s
/// `.git`.
fn names(admin: &Path, workspace: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(admin.join("gitdir")) else {
        return false;
    };
    let named = admin.join(text.trim());
    same(&named, &workspace.join(".git"))
}

/// Whether `admin` is a folder right under `common`'s worktrees.
fn under_worktrees(common: &Path, admin: &Path) -> bool {
    admin
        .parent()
        .is_some_and(|parent| same(parent, &common.join(WORKTREES)))
}

/// Whether `a` and `b` are one path, compared by [`canonical`].
fn same(a: &Path, b: &Path) -> bool {
    a == b || canonical(a) == canonical(b)
}

/// `path` with its longest existing start made canonical: so a path
/// that is gone still compares with the one recorded while it was
/// there.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => canonical(parent).join(name),
        _ => path.to_owned(),
    }
}

/// Whether `dir` is a folder with nothing in it.
fn is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
}

/// Counts a file or folder that is gone already as removed.
fn not_found(error: io::Error) -> io::Result<()> {
    if error.kind() == io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_a_short_ref_name_component() {
        assert_eq!(id_for("chat-1"), "chat-1");
        assert_eq!(id_for("a b/c.d"), "a-b-c-d");
        let long = format!("{}-sub-0123456789ab", "w".repeat(80));
        let id = id_for(&long);
        assert!(id.len() <= MAX_ID);
        assert!(id.ends_with("-sub-0123456789ab"));
        assert_eq!(id_for("..."), "run");
    }
}
