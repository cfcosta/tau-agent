//! Pushing a repository's main chat to GitHub (ADR 0023): trunk's
//! commits go as they are, through jj-lib's push, which runs `git`.

use super::*;
use crate::push::{PushFailure, Pushed, PushedChange};

impl Host {
    /// Where `repo` pushes: its GitHub repository's URL, and the token
    /// of the GitHub sign-in.
    pub(super) fn github_remote(
        &self,
        repo: &str,
    ) -> anyhow::Result<(String, String)> {
        let full_name = self.github_of(repo).ok_or_else(|| {
            anyhow::anyhow!("{repo} was not cloned from GitHub")
        })?;
        let token = github::Token::load(&self.config.credentials)
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
        let project = slot.project().await.map_err(failed)?;
        let (url, token) = self.github_remote(repo).map_err(failed)?;
        if fetch {
            let main = self.main_of(repo).await.map_err(failed)?;
            if self.is_running(&main) {
                return Err(failed(anyhow::anyhow!(
                    "main is running: fetch and push once its turn ends"
                )));
            }
            self.update_repo(repo).await.map_err(failed)?;
            self.catch_up(&project, DEFAULT_WORKSPACE)
                .await
                .map_err(failed)?;
        }
        // The push and, when GitHub moved, what main has that it lacks.
        let (pushed, ahead) = project
            .run(move |project| {
                let remote = tau_vcs::Remote {
                    url: &url,
                    token: Some(&token),
                };
                let pushed = project.push_trunk(remote);
                let ahead = matches!(
                    pushed,
                    Err(tau_vcs::VcsError::PushRejected { .. })
                )
                .then(|| {
                    project.unpushed().map_or(0, |changes| changes.len() as u32)
                });
                (pushed, ahead)
            })
            .await;
        match pushed {
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
            Err(tau_vcs::VcsError::PushRejected { branch }) => {
                Err(PushFailure::Moved {
                    branch,
                    ahead: ahead.unwrap_or_default(),
                })
            }
            Err(error) => Err(failed(error.into())),
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
