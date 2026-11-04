//! Pushing a repository's main chat to GitHub (ADR 0023): trunk's
//! commits go as they are, through jj-lib's push, which runs `git`.

use tau_ui_plugin::vcs::PushError;

use super::*;
use crate::push::{PushFailure, Pushed, PushedChange};

impl Host {
    /// Where `repo` pushes: its GitHub repository's URL, and the token
    /// of the GitHub sign-in.
    pub(super) async fn github_remote(
        &self,
        repo: &str,
    ) -> anyhow::Result<(String, String)> {
        if let Some(error) = self.local_has_no_remote(repo) {
            return Err(error);
        }
        let full_name = self.github_of(repo).ok_or_else(|| {
            anyhow::anyhow!("{repo} was not cloned from GitHub")
        })?;
        let token = github::Token::load(&self.config.credentials)
            .await
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        Ok((self.github.clone_url(&full_name), token.token))
    }

    /// Pushes `repo`'s trunk, its main chat's commits, to GitHub's
    /// branch of the same name. With `fetch`, it first brings in what
    /// GitHub has (`update_repo`) and catches the main chat up, which
    /// puts its commits on top of GitHub's new ones; the main chat must
    /// not be running then. A push that finds GitHub's branch moved
    /// since the last fetch is [`PushFailure::Moved`].
    pub async fn push_main(
        &self,
        repo: &str,
        fetch: bool,
    ) -> Result<Pushed, PushFailure> {
        let failed =
            |error: anyhow::Error| PushFailure::Failed(format!("{error:#}"));
        let slot = self
            .slot(repo)
            .ok_or_else(|| failed(anyhow::anyhow!("No repository {repo}")))?;
        let repository = slot.repository().await.map_err(failed)?;
        let (url, token) = self.github_remote(repo).await.map_err(failed)?;
        if fetch {
            let main = self.main_of(repo).await.map_err(failed)?;
            if self.is_running(&main) {
                return Err(failed(anyhow::anyhow!(
                    "main is running: fetch and push once its turn ends"
                )));
            }
            self.update_repo(repo).await.map_err(failed)?;
            repository
                .catch_up(repository.main_workspace())
                .await
                .map_err(failed)?;
        }
        let remote = Remote {
            url,
            token: Some(token),
        };
        match repository.push_trunk(remote).await {
            Ok(pushed) => Ok(Pushed {
                branch: pushed.branch,
                from: pushed.from,
                to: pushed.to,
                changes: pushed
                    .changes
                    .into_iter()
                    .map(|change| PushedChange {
                        change_id: change.change_id,
                        title: first_line(&change.description),
                    })
                    .collect(),
            }),
            Err(PushError::Moved { branch, ahead }) => {
                Err(PushFailure::Moved { branch, ahead })
            }
            Err(PushError::Failed(error)) => Err(failed(error)),
        }
    }
}

/// A description's first line, or `(no description set)`.
fn first_line(description: &str) -> String {
    match description.lines().next().map(str::trim) {
        Some(line) if !line.is_empty() => line.to_owned(),
        _ => "(no description set)".to_owned(),
    }
}
