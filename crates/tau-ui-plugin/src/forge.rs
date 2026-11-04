//! Where repositories are hosted (ADR 0045): what the host clones from,
//! pushes to and opens pull requests on, through the [`FORGE`] slot's
//! plugin (tau-github). A repository there is named by its full name,
//! such as `owner/name`.

use serde::{Deserialize, Serialize};

use crate::{registry::HostFuture, slots::Slot, vcs::Remote};

/// The slot the host reaches where repositories are hosted through.
pub const FORGE: Slot<dyn Forge> = Slot::new("tau.forge");

/// A repository the signed-in account reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeRepo {
    /// Its full name, such as `owner/name`.
    pub name: String,
    pub description: String,
    /// Its default branch.
    pub branch: String,
    /// When it was last pushed to, as the forge writes it; empty when
    /// it does not say.
    pub pushed_at: String,
    /// The URL it clones from, when the forge says.
    pub remote: Option<String>,
}

/// A pull request to open: its branch `head` onto `base` in `repo`, and
/// who to ask for reviews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPull {
    /// The repository's full name.
    pub repo: String,
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
    pub reviewers: Vec<String>,
}

/// A pull request the forge opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub number: u64,
    pub url: String,
}

/// How the checks on a commit stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checks {
    Running,
    Passed,
    Failed,
    /// The repository runs no checks.
    None,
}

/// Where repositories are hosted: what fills [`FORGE`].
pub trait Forge: Send + Sync {
    /// Its name, for labels: `GitHub`.
    fn name(&self) -> &str;

    /// A name for its clones' directory under tau's repositories, such
    /// as `github`.
    fn id(&self) -> &str;

    /// The URL `full_name` clones from.
    fn clone_url(&self, full_name: &str) -> String;

    /// The full name of the repository the Git URL `url` names, when it
    /// is one of the forge's, in any form Git takes (HTTPS, SSH, …).
    fn repository_of(&self, url: &str) -> Option<String>;

    /// The account signed in, if any.
    fn signed_in(&self) -> HostFuture<'_, Option<String>>;

    /// Where `full_name` fetches and pushes, with the sign-in's token;
    /// an error that says to sign in when no one is.
    fn remote<'a>(
        &'a self,
        full_name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Remote>>;

    /// Whether the sign-in can push to `full_name`; false signed out.
    fn can_push<'a>(
        &'a self,
        full_name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<bool>>;

    /// The repositories the sign-in reaches, the most recently pushed
    /// first.
    fn repos(&self) -> HostFuture<'_, anyhow::Result<Vec<ForgeRepo>>>;

    /// Opens `pull`, and asks its reviewers for reviews.
    fn open_pull(
        &self,
        pull: OpenPull,
    ) -> HostFuture<'_, anyhow::Result<Opened>>;

    /// How the checks on the commit `head` of `full_name` stand.
    fn checks<'a>(
        &'a self,
        full_name: &'a str,
        head: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Checks>>;
}
