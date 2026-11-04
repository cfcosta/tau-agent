//! The GitHub repository a Git URL names, such as a local repository's
//! `origin`.

use gix::bstr::{BStr, ByteSlice as _};

/// A repository on GitHub: `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub owner: String,
    pub name: String,
}

impl Repository {
    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// The GitHub repository `url` names: an HTTPS, HTTP, `git://` or SSH
/// URL, or Git's `git@github.com:owner/name` form, on `github.com`
/// (`www.` and `ssh.` too), with or without `.git` and a closing `/`.
pub fn repository_of(url: &str) -> Option<Repository> {
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
    (owner_ok && name_ok).then(|| Repository {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}
