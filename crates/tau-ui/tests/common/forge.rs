//! A forge for tests, in place of GitHub: it clones and pushes at a
//! `file://` folder of bare repositories, says who is signed in and
//! where the sign-in can push, opens pull requests and answers their
//! checks, and keeps what it was asked.

use std::sync::Mutex;

use tau_ui_plugin::{
    forge::{Checks, Forge, ForgeRepo, OpenPull, Opened},
    registry::HostFuture,
    vcs::Remote,
};

/// The token a signed-in fake hands out with its remotes.
pub const TOKEN: &str = "ghu_token";

/// A fake GitHub whose repositories are `<web>/<owner>/<name>.git`.
pub struct FakeForge {
    web: String,
    user: Mutex<Option<String>>,
    /// Whether the sign-in can push to each repository.
    push: bool,
    /// The pull requests it opened, in order.
    pulls: Mutex<Vec<OpenPull>>,
    /// What it was asked that reaches GitHub's API, in order.
    asked: Mutex<Vec<String>>,
}

impl FakeForge {
    /// Signed out, its repositories under `web`; the sign-in, once there
    /// is one, can push to them.
    pub fn at(web: impl Into<String>) -> Self {
        Self {
            web: web.into(),
            user: Mutex::new(None),
            push: true,
            pulls: Mutex::default(),
            asked: Mutex::default(),
        }
    }

    /// Signed in as `user`.
    pub fn signed_in(self, user: &str) -> Self {
        self.sign_in(user);
        self
    }

    /// Whether the sign-in can push.
    pub fn pushing(mut self, push: bool) -> Self {
        self.push = push;
        self
    }

    /// Signs in as `user`, from now on.
    pub fn sign_in(&self, user: &str) {
        *self.user.lock().unwrap() = Some(user.to_owned());
    }

    /// The pull requests it opened.
    pub fn pulls(&self) -> Vec<OpenPull> {
        self.pulls.lock().unwrap().clone()
    }

    /// What it was asked of GitHub's API: `open_pull owner/name`,
    /// `checks owner/name head`, …
    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }

    fn ask(&self, what: String) {
        self.asked.lock().unwrap().push(what);
    }

    fn user(&self) -> Option<String> {
        self.user.lock().unwrap().clone()
    }

    fn token(&self) -> anyhow::Result<String> {
        self.user()
            .map(|_| TOKEN.to_owned())
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))
    }
}

/// `owner/name` of a GitHub URL, SSH or HTTPS, with or without `.git`.
fn github_repository(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .then(|| format!("{owner}/{name}"))
}

impl Forge for FakeForge {
    fn name(&self) -> &str {
        "GitHub"
    }

    fn id(&self) -> &str {
        "github"
    }

    fn clone_url(&self, full_name: &str) -> String {
        format!("{}/{full_name}.git", self.web)
    }

    fn repository_of(&self, url: &str) -> Option<String> {
        let own = url
            .strip_prefix(&format!("{}/", self.web))
            .and_then(|path| path.strip_suffix(".git"))
            .map(str::to_owned);
        own.or_else(|| github_repository(url))
    }

    fn signed_in(&self) -> HostFuture<'_, Option<String>> {
        Box::pin(async move { self.user() })
    }

    fn remote<'a>(
        &'a self,
        full_name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Remote>> {
        Box::pin(async move {
            Ok(Remote {
                url: self.clone_url(full_name),
                token: Some(self.token()?),
            })
        })
    }

    fn can_push<'a>(
        &'a self,
        full_name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<bool>> {
        Box::pin(async move {
            self.ask(format!("can_push {full_name}"));
            Ok(self.user().is_some() && self.push)
        })
    }

    fn repos(&self) -> HostFuture<'_, anyhow::Result<Vec<ForgeRepo>>> {
        Box::pin(async move {
            // None that the tests list.
            self.token()?;
            Ok(Vec::new())
        })
    }

    fn open_pull(
        &self,
        pull: OpenPull,
    ) -> HostFuture<'_, anyhow::Result<Opened>> {
        Box::pin(async move {
            self.token()?;
            self.ask(format!("open_pull {}", pull.repo));
            let mut pulls = self.pulls.lock().unwrap();
            pulls.push(pull.clone());
            let number = 6 + pulls.len() as u64;
            Ok(Opened {
                number,
                url: format!("https://github.com/{}/pull/{number}", pull.repo),
            })
        })
    }

    fn checks<'a>(
        &'a self,
        full_name: &'a str,
        head: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Checks>> {
        Box::pin(async move {
            self.token()?;
            self.ask(format!("checks {full_name} {head}"));
            Ok(Checks::Passed)
        })
    }
}
