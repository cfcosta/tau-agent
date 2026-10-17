//! The remote of the repository a folder is in, read from its Git
//! config without running Git: a Git checkout (`.git` a directory, or a
//! file pointing at one), or a jj repository, whose Git store
//! `.jj/repo/store/git_target` names. `origin` when there is one, else
//! the first remote listed.

use std::path::{Path, PathBuf};

/// The remote URL of the repository holding `folder`, as its config
/// writes it; `None` when it is in none, or has no remote.
pub fn of_folder(folder: &Path) -> Option<String> {
    folder
        .ancestors()
        .find_map(git_dir)
        .and_then(|dir| std::fs::read_to_string(dir.join("config")).ok())
        .and_then(|config| from_config(&config))
}

/// The Git directory of the repository whose top is `dir`, if it is
/// one: the one whose config holds the remotes.
fn git_dir(dir: &Path) -> Option<PathBuf> {
    let dot_git = dir.join(".git");
    if dot_git.is_dir() {
        return Some(common(dot_git));
    }
    if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let target = text.trim().strip_prefix("gitdir:")?.trim();
        return Some(common(dir.join(target)));
    }
    let mut repo = dir.join(".jj").join("repo");
    if repo.is_file() {
        // A secondary jj workspace: the file names the repository.
        let text = std::fs::read_to_string(&repo).ok()?;
        repo = dir.join(".jj").join(text.trim());
    }
    let store = repo.join("store");
    let target = std::fs::read_to_string(store.join("git_target")).ok()?;
    Some(store.join(target.trim()))
}

/// A worktree's Git directory names the shared one in `commondir`.
fn common(git: PathBuf) -> PathBuf {
    match std::fs::read_to_string(git.join("commondir")) {
        Ok(text) => git.join(text.trim()),
        Err(_) => git,
    }
}

/// The URL of `origin` in a Git config, else of the first remote.
pub fn from_config(config: &str) -> Option<String> {
    let mut remotes: Vec<(String, String)> = Vec::new();
    let mut section: Option<String> = None;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(header) =
            line.strip_prefix('[').and_then(|l| l.strip_suffix(']'))
        {
            section = header
                .trim()
                .strip_prefix("remote")
                .map(|name| name.trim().trim_matches('"').to_owned())
                .filter(|name| !name.is_empty());
            continue;
        }
        let Some(name) = &section else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("url") {
            let value = value.trim().trim_matches('"').to_owned();
            if !remotes.iter().any(|(each, _)| each == name) {
                remotes.push((name.clone(), value));
            }
        }
    }
    remotes
        .iter()
        .find(|(name, _)| name == "origin")
        .or_else(|| remotes.first())
        .map(|(_, url)| url.clone())
}
