//! Opening a chat's work as a pull request, and pushing its later
//! commits to it (ADR 0023): the chat's own commits, replayed onto
//! GitHub's trunk, go to the pull request's branch through jj-lib's
//! push. GitHub's API opens the pull request, asks for reviews and
//! reports checks.

use tau_ui_plugin::vcs::After;

use super::*;

/// A pull request opened from a run, as far as tau pushed it.
#[derive(Debug, Clone)]
pub(super) struct OpenPr {
    pub(super) repo: String,
    pub(super) branch: String,
    pub(super) keep_pushing: bool,
    /// The branch's commit on GitHub, and the change id of the run's
    /// last commit in it.
    pub(super) head: String,
    pub(super) change: String,
}

impl Host {
    /// Writes a pull request draft from `run`: its commits replayed on
    /// its repository's default branch, its prompt as the title and its
    /// last answer as the description. A main chat has none: it pushes
    /// to GitHub itself.
    pub async fn prepare_pull_request(
        &self,
        run: &RunId,
    ) -> anyhow::Result<PullRequest> {
        if self.is_main(run) {
            anyhow::bail!(
                "main pushes to GitHub itself: use Push to GitHub instead of \
                 a pull request"
            );
        }
        let slot = self.slot_of_run(run).await?;
        if let Some(error) = self.local_has_no_remote(&slot.name) {
            return Err(error);
        }
        let repo = self.github_of(&slot.name).ok_or_else(|| {
            anyhow::anyhow!(
                "Pull requests need a repository cloned from GitHub; {} was \
                 not",
                slot.name
            )
        })?;
        let repository = slot.repository().await?;
        let (url, token) = self.github_remote(&slot.name).await?;
        // What GitHub's default branch is now, for the replay to go on.
        let remote = Remote {
            url,
            token: Some(token),
        };
        let _ = repository.update(remote).await;
        let replayed = repository.replay_run(run, None).await?;
        let commits = replayed
            .commits
            .iter()
            .map(|commit| PrCommit {
                title: commit
                    .change
                    .description
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                added: commit.added,
                removed: commit.removed,
            })
            .collect::<Vec<_>>();
        let changed = replayed.changed;
        let base = repository.trunk_name().await?;
        let view = self
            .history()
            .await?
            .into_iter()
            .find(|view| &view.id == run);
        let prompt = view
            .as_ref()
            .and_then(|view| {
                view.items.iter().find_map(|item| match item {
                    crate::view::Item::User(text) => Some(text.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        let answer = view
            .as_ref()
            .and_then(|view| view.last_text().map(str::to_owned))
            .unwrap_or_default();
        let turns = view.as_ref().map_or(0, |view| view.turn);
        let head = format!(
            "tau/{}-{}",
            branch_slug(&prompt),
            run.0
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
        let draft = PullRequest {
            repo: repo.clone(),
            head,
            base,
            // The replay went through: the commits apply on GitHub's
            // branch as it was fetched.
            mergeable: true,
            summary: format!(
                "{} changed {changed} files over {turns} turns.",
                self.title_of(run).await?,
            ),
            tests: tests_passed(&answer),
            title: pr_title(&prompt),
            body: format!(
                "{answer}\n\n---\nMade with tau from run `{}`.",
                run.0
            ),
            commits,
            draft: true,
            keep_pushing: true,
            state: PrState::Draft,
        };
        self.drafts
            .lock()
            .expect("not poisoned")
            .insert(run.clone(), draft.clone());
        Ok(draft)
    }

    /// The draft written for `run`, if any.
    pub fn draft(&self, run: &RunId) -> Option<PullRequest> {
        self.drafts.lock().expect("not poisoned").get(run).cloned()
    }

    /// Pushes the run's commits, replayed on GitHub's trunk, to the
    /// draft's branch, and opens the pull request, asking `reviewers` to
    /// review it. Returns it and the branch's commit.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_pull_request(
        &self,
        run: &RunId,
        draft: &PullRequest,
        title: &str,
        body: &str,
        as_draft: bool,
        keep_pushing: bool,
        reviewers: &[String],
    ) -> anyhow::Result<(github::Opened, String)> {
        let slot = self.slot_of_run(run).await?;
        let repository = slot.repository().await?;
        let (url, token) = self.github_remote(&slot.name).await?;
        let replayed = repository.replay_run(run, None).await?;
        let last = replayed.commits.last();
        let copy = last.map(|commit| commit.copy.clone()).unwrap_or_default();
        let change = last
            .map(|commit| commit.change.change_id.clone())
            .unwrap_or_default();
        let remote = Remote {
            url,
            token: Some(token.clone()),
        };
        // Signing on push makes copies of its own: the head is what went.
        let head = repository
            .push_branch(remote, draft.head.clone(), None, copy)
            .await?;
        let opened = self
            .github
            .open_pull(
                &token,
                &draft.repo,
                title,
                body,
                &draft.head,
                &draft.base,
                as_draft,
            )
            .await
            .map_err(anyhow::Error::msg)?;
        self.github
            .request_reviewers(&token, &draft.repo, opened.number, reviewers)
            .await
            .map_err(anyhow::Error::msg)?;
        self.prs.lock().expect("not poisoned").insert(
            run.clone(),
            OpenPr {
                repo: draft.repo.clone(),
                branch: draft.head.clone(),
                keep_pushing,
                head: head.clone(),
                change,
            },
        );
        Ok((opened, head))
    }

    /// Pushes the commits `run` made since its pull request's last
    /// push, if it has one that keeps pushing: replayed onto the
    /// branch's commit, a fast-forward. Returns whether it pushed.
    pub async fn push_later_commits(
        &self,
        run: &RunId,
    ) -> anyhow::Result<bool> {
        let Some(open) =
            self.prs.lock().expect("not poisoned").get(run).cloned()
        else {
            return Ok(false);
        };
        if !open.keep_pushing {
            return Ok(false);
        }
        let slot = self.slot_of_run(run).await?;
        let repository = slot.repository().await?;
        let (url, token) = self.github_remote(&slot.name).await?;
        let after = After {
            head: open.head.clone(),
            change_id: open.change.clone(),
        };
        let replayed = repository.replay_run(run, Some(after)).await?;
        let Some(last) = replayed.commits.last() else {
            return Ok(false);
        };
        let remote = Remote {
            url,
            token: Some(token),
        };
        let head = repository
            .push_branch(
                remote,
                open.branch.clone(),
                Some(open.head.clone()),
                last.copy.clone(),
            )
            .await?;
        let change = last.change.change_id.clone();
        if let Some(open) = self.prs.lock().expect("not poisoned").get_mut(run)
        {
            open.head = head;
            open.change = change;
        }
        Ok(true)
    }

    /// Whether `run` has a pull request that takes its later turns.
    pub fn keeps_pushing(&self, run: &RunId) -> bool {
        self.prs
            .lock()
            .expect("not poisoned")
            .get(run)
            .is_some_and(|open| open.keep_pushing)
    }

    /// How the checks on an opened pull request's head stand.
    pub async fn pull_request_checks(
        &self,
        run: &RunId,
    ) -> anyhow::Result<crate::pull_request::Checks> {
        let open = self
            .prs
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No pull request for the run"))?;
        let token = github::Token::load(&self.config.credentials)
            .await
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        self.github
            .checks(&token.token, &open.repo, &open.head)
            .await
            .map_err(anyhow::Error::msg)
    }
}

/// A pull request's title from the run's prompt: its first line, up to
/// 72 characters, with a capital first letter and no final period.
pub(super) fn pr_title(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default().trim();
    let mut title: String = line.chars().take(72).collect();
    if line.chars().count() > 72 {
        title = title.trim_end().to_owned() + "…";
    }
    let title = title.trim_end_matches('.').to_owned();
    let mut chars = title.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Changes from tau".into(),
    }
}

/// `14 tests passed`, from an answer that says how many passed.
pub(super) fn tests_passed(answer: &str) -> Option<String> {
    let words: Vec<&str> = answer.split_whitespace().collect();
    words.windows(2).find_map(|pair| {
        let count: u32 = pair[0]
            .trim_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .ok()?;
        pair[1]
            .starts_with("passed")
            .then(|| format!("{count} tests passed"))
    })
}

/// Asks GitHub about an opened pull request's checks until they are
/// done, for half an hour at most, and shows each change.
pub(super) async fn watch_checks(
    host: Arc<Host>,
    run: RunId,
    workspace: gpui::WeakEntity<Workspace>,
    cx: &mut gpui::AsyncApp,
) {
    use crate::pull_request::Checks;
    for _ in 0..120 {
        let job = {
            let (checker, run) = (host.clone(), run.clone());
            host.runtime
                .spawn(async move { checker.pull_request_checks(&run).await })
        };
        let Ok(Ok(checks)) = job.await else {
            return;
        };
        let updated = workspace.update(cx, |ws, cx| {
            if let Some(pr) = ws.pull_request(&run).cloned()
                && let PrState::Opened { number, url, .. } = pr.state
            {
                ws.apply(
                    HostUpdate::PullRequestState {
                        run: run.clone(),
                        state: PrState::Opened {
                            number,
                            url,
                            checks,
                        },
                    },
                    cx,
                );
            }
        });
        if updated.is_err() || checks != Checks::Running {
            return;
        }
        cx.background_executor()
            .timer(std::time::Duration::from_secs(15))
            .await;
    }
}
