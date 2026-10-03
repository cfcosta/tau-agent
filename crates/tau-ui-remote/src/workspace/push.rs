//! Pushing a repository's main chat to GitHub (ADR 0023).

use super::*;
use crate::push::{PushFailure, Pushed};

impl Workspace {
    /// The repository whose main chat `run` is, if it is one.
    pub fn main_repo(&self, run: &RunId) -> Option<&str> {
        self.catalog
            .repos
            .iter()
            .find(|repo| repo.main.as_ref() == Some(run))
            .map(|repo| repo.name.as_str())
    }

    /// How many of `repo`'s trunk changes GitHub lacks, and trunk's
    /// branch; `None` when there is nothing to push or no GitHub sign-in
    /// to push with.
    pub fn unpushed(&self, repo: &str) -> Option<(u32, &str)> {
        let repo = self.catalog.repo(repo)?;
        (self.catalog.pull_requests && repo.unpushed > 0)
            .then(|| (repo.unpushed, repo.trunk.as_deref().unwrap_or("main")))
    }

    pub fn push_state(&self, repo: &str) -> Option<&PushState> {
        self.pushes.get(repo)
    }

    /// Pushes `repo`'s trunk to GitHub; fetching first when `fetch`.
    pub fn push(&mut self, repo: &str, fetch: bool, cx: &mut Context<Self>) {
        if matches!(self.pushes.get(repo), Some(PushState::Pushing { .. })) {
            return;
        }
        self.pushes
            .insert(repo.to_owned(), PushState::Pushing { fetching: fetch });
        self.follow = true;
        cx.emit(WorkspaceEvent::Push {
            repo: repo.to_owned(),
            fetch,
        });
        cx.notify();
    }

    /// What a push of `repo` came to: a card in its main chat for a
    /// push that went or one GitHub's moved branch refused; a dialog
    /// for anything else.
    pub fn pushed(
        &mut self,
        repo: &str,
        result: Result<Pushed, PushFailure>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(pushed) => {
                if let Some(listed) = self.catalog.repo_mut(repo) {
                    listed.unpushed = 0;
                }
                self.pushes
                    .insert(repo.to_owned(), PushState::Pushed(pushed));
            }
            Err(PushFailure::Moved { branch, ahead }) => {
                self.pushes.insert(
                    repo.to_owned(),
                    PushState::Moved { branch, ahead },
                );
            }
            Err(PushFailure::Failed(error)) => {
                self.pushes.remove(repo);
                self.show_alert(format!("Could not push {repo}"), error, cx);
            }
        }
        cx.notify();
    }
}
