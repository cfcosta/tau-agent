//! GitHub as the `tau.forge` slot's filler: the sign-in saved in tau's
//! config directory, and GitHub's API with it.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tau_ui_plugin::{
    HostCx,
    PluginHost,
    forge::{Checks, Forge, ForgeRepo, OpenPull, Opened},
    registry::HostFuture,
    vcs::Remote,
};

use crate::{Api, Endpoints, Token, sign_in::Flow};

/// GitHub, signed in with the token saved in tau's config directory.
#[derive(Clone)]
pub struct GitHub {
    api: Api,
    /// tau's config directory, which keeps the sign-in.
    dir: PathBuf,
    /// The sign-in going on, shared by every clone.
    pub(crate) flow: Arc<Flow>,
}

impl GitHub {
    /// GitHub at `endpoints`, with the sign-in kept in `dir`.
    pub fn new(dir: impl Into<PathBuf>, endpoints: &Endpoints) -> Self {
        Self {
            api: Api::of(endpoints),
            dir: dir.into(),
            flow: Arc::default(),
        }
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    /// tau's config directory, which keeps the sign-in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The saved sign-in, or an error that says to sign in.
    async fn token(&self) -> anyhow::Result<Token> {
        Token::load(&self.dir)
            .await
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))
    }
}

impl PluginHost for GitHub {
    /// Keeps the sign-in in the host's config directory, and reaches
    /// GitHub at the host's [`Endpoints`], or GitHub's own.
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let dir = cx.config_dir().ok_or_else(|| {
            anyhow::anyhow!("the host has no config directory to sign in in")
        })?;
        let endpoints =
            cx.services.get::<Endpoints>().cloned().unwrap_or_default();
        Ok(Self::new(dir, &endpoints))
    }
}

impl Forge for GitHub {
    fn name(&self) -> &str {
        "GitHub"
    }

    fn id(&self) -> &str {
        "github"
    }

    fn clone_url(&self, full_name: &str) -> String {
        self.api.clone_url(full_name)
    }

    fn repository_of(&self, url: &str) -> Option<String> {
        self.api.https_of(url).map(|(full_name, _)| full_name)
    }

    fn signed_in(&self) -> HostFuture<'_, Option<String>> {
        Box::pin(async move { Token::load(&self.dir).await.map(|t| t.user) })
    }

    fn remote<'a>(
        &'a self,
        full_name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Remote>> {
        Box::pin(async move {
            let token = self.token().await?;
            Ok(Remote {
                url: self.api.clone_url(full_name),
                token: Some(token.token),
            })
        })
    }

    fn can_push<'a>(
        &'a self,
        full_name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<bool>> {
        Box::pin(async move {
            let Some(token) = Token::load(&self.dir).await else {
                return Ok(false);
            };
            self.api
                .can_push(&token.token, full_name)
                .await
                .map_err(anyhow::Error::msg)
        })
    }

    fn repos(&self) -> HostFuture<'_, anyhow::Result<Vec<ForgeRepo>>> {
        Box::pin(async move {
            let token = self.token().await?;
            self.api
                .repos(&token.token)
                .await
                .map_err(anyhow::Error::msg)
        })
    }

    fn open_pull(
        &self,
        pull: OpenPull,
    ) -> HostFuture<'_, anyhow::Result<Opened>> {
        Box::pin(async move {
            let token = self.token().await?;
            let opened = self
                .api
                .open_pull(
                    &token.token,
                    &pull.repo,
                    &pull.title,
                    &pull.body,
                    &pull.head,
                    &pull.base,
                    pull.draft,
                )
                .await
                .map_err(anyhow::Error::msg)?;
            self.api
                .request_reviewers(
                    &token.token,
                    &pull.repo,
                    opened.number,
                    &pull.reviewers,
                )
                .await
                .map_err(anyhow::Error::msg)?;
            Ok(opened)
        })
    }

    fn checks<'a>(
        &'a self,
        full_name: &'a str,
        head: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Checks>> {
        Box::pin(async move {
            let token = self.token().await?;
            self.api
                .checks(&token.token, full_name, head)
                .await
                .map_err(anyhow::Error::msg)
        })
    }
}
