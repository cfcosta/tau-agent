//! Pushing a repository's main chat to the forge (ADR 0023): trunk's
//! commits go as they are, through the VCS plugin's push.

use tau_ui_plugin::vcs::PushError;

use super::*;
use crate::push::{PushFailure, Pushed, PushedChange};

impl Host {
    /// Where `repo` pushes: its repository on the forge, with the
    /// forge's sign-in.
    pub(super) async fn forge_remote(
        &self,
        repo: &str,
    ) -> anyhow::Result<Remote> {
        if let Some(error) = self.local_has_no_remote(repo) {
            return Err(error);
        }
        let forge = self.forge.as_ref().ok_or_else(no_forge)?;
        let full_name = self.forge_of(repo).ok_or_else(|| {
            anyhow::anyhow!("{repo} was not cloned from {}", forge.name())
        })?;
        forge.remote(&full_name).await
    }

    /// Pushes `repo`'s trunk, its main chat's commits, to the forge's
    /// branch of the same name. With `fetch`, it first brings in what
    /// the forge has (`update_repo`) and catches the main chat up, which
    /// puts its commits on top of the forge's new ones; the main chat
    /// must not be running then. A push that finds the forge's branch
    /// moved since the last fetch is [`PushFailure::Moved`].
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
        let remote = self.forge_remote(repo).await.map_err(failed)?;
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
