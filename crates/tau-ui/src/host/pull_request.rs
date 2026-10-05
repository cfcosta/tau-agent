//! Opening a chat's work as a pull request, and pushing its later
//! commits to it (ADR 0023): the chat's own commits, replayed onto
//! GitHub's trunk, go to the pull request's branch through jj-lib's
//! push. GitHub's API opens the pull request, asks for reviews and
//! reports checks.

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

/// A run's commits replayed for its pull request: the commit they went
/// onto, the run's commits, oldest first, and their copies.
pub(super) struct Replayed {
    pub(super) onto: String,
    pub(super) changes: Vec<tau_vcs::StackChange>,
    pub(super) copies: Vec<String>,
}

impl Host {
    /// The commits on `run`'s stack, oldest first: the model's, and
    /// those its children landed (ADR 0014).
    pub(super) fn stack(
        &self,
        run: &RunId,
        project: &Project,
    ) -> anyhow::Result<Vec<tau_vcs::StackChange>> {
        let Some(head) = project.blocking().bookmark(&bookmark(run))? else {
            return Ok(Vec::new());
        };
        Ok(project.blocking().stack(&head)?)
    }

    /// `run`'s commits replayed onto GitHub's trunk as the last fetch
    /// left it, so the pull request carries only the run's own work and
    /// none of what the main chat has not pushed. `after` is the pull
    /// request's branch and the change id of the run's last commit in
    /// it: only the commits after that one replay, onto the branch. A
    /// replay that would conflict is refused, naming the files.
    pub(super) fn replayed(
        &self,
        run: &RunId,
        project: &Project,
        after: Option<(&str, &str)>,
    ) -> anyhow::Result<Replayed> {
        let all = self.stack(run, project)?;
        if all.is_empty() {
            anyhow::bail!(
                "The run has no commits, so there is nothing to propose"
            );
        }
        let trunk = project.blocking().trunk_name()?;
        let upstream = || {
            project.blocking().upstream()?.ok_or_else(|| {
                anyhow::anyhow!(
                    "GitHub has no {trunk} for the pull request to go on"
                )
            })
        };
        let (onto, changes) = match after {
            Some((head, pushed)) => {
                // Changes keep their ids when a landing restacks them.
                match all.iter().position(|change| change.change_id == pushed) {
                    Some(at) => (head.to_owned(), all[at + 1..].to_vec()),
                    None => (upstream()?, all),
                }
            }
            None => (upstream()?, all),
        };
        let ids: Vec<String> = changes
            .iter()
            .map(|change| change.commit_id.clone())
            .collect();
        let copies = project.blocking().replay(&ids, &onto).map_err(
            |error| match error {
                tau_vcs::VcsError::WouldConflict(paths) => anyhow::anyhow!(
                    "would conflict on origin/{trunk}: {}",
                    paths.join(", ")
                ),
                error => error.into(),
            },
        )?;
        Ok(Replayed {
            onto,
            changes,
            copies,
        })
    }

    /// Writes a pull request draft from `run`: its commits replayed on
    /// its repository's default branch, its prompt as the title and its
    /// last answer as the description. A main chat has none: it pushes
    /// to GitHub itself.
    pub fn prepare_pull_request(
        &self,
        run: &RunId,
    ) -> anyhow::Result<PullRequest> {
        if self.is_main(run) {
            anyhow::bail!(
                "main pushes to GitHub itself: use Push to GitHub instead of \
                 a pull request"
            );
        }
        let slot = self.slot_of_run(run)?;
        let repo = self.github_of(&slot.name).ok_or_else(|| {
            anyhow::anyhow!(
                "Pull requests need a repository cloned from GitHub; {} was \
                 not",
                slot.name
            )
        })?;
        let (url, token) = self.github_remote(&slot.name)?;
        let project = slot
            .project
            .wait()
            .ok_or_else(|| anyhow::anyhow!("{} has no project", slot.name))?;
        // What GitHub's default branch is now, for the replay to go on.
        let _ = project.blocking().update(tau_vcs::UpdateFrom::Remote {
            url: &url,
            token: Some(&token),
        });
        let replayed = self.replayed(run, &project, None)?;
        let last = replayed.copies.last().expect("a replay of some commits");
        let changed = project.blocking().diff(&replayed.onto, last)?.len();
        let mut commits = Vec::new();
        let mut previous = replayed.onto.clone();
        for (change, copy) in replayed.changes.iter().zip(&replayed.copies) {
            let files = project.blocking().diff(&previous, copy)?;
            commits.push(PrCommit {
                title: change
                    .description
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                added: files.iter().map(|file| file.added as u32).sum(),
                removed: files.iter().map(|file| file.removed as u32).sum(),
            });
            previous = copy.clone();
        }
        let view = self.history()?.into_iter().find(|view| &view.id == run);
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
            base: project.blocking().trunk_name()?,
            // The replay went through: the commits apply on GitHub's
            // branch as it was fetched.
            mergeable: true,
            summary: format!(
                "{} changed {changed} files over {turns} turns.",
                self.title_of(run)?,
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
    pub fn create_pull_request(
        &self,
        run: &RunId,
        draft: &PullRequest,
        title: &str,
        body: &str,
        as_draft: bool,
        keep_pushing: bool,
        reviewers: &[String],
    ) -> anyhow::Result<(github::Opened, String)> {
        let slot = self.slot_of_run(run)?;
        let project = slot.project()?;
        let (url, token) = self.github_remote(&slot.name)?;
        let replayed = self.replayed(run, &project, None)?;
        let head = replayed.copies.last().cloned().unwrap_or_default();
        let change = replayed
            .changes
            .last()
            .map(|change| change.change_id.clone())
            .unwrap_or_default();
        let remote = tau_vcs::Remote {
            url: &url,
            token: Some(&token),
        };
        project
            .blocking()
            .push_branch(remote, &draft.head, None, &head)?;
        let opened = self
            .runtime
            .block_on(self.github.open_pull(
                &token,
                &draft.repo,
                title,
                body,
                &draft.head,
                &draft.base,
                as_draft,
            ))
            .map_err(anyhow::Error::msg)?;
        self.runtime
            .block_on(self.github.request_reviewers(
                &token,
                &draft.repo,
                opened.number,
                reviewers,
            ))
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
    pub fn push_later_commits(&self, run: &RunId) -> anyhow::Result<bool> {
        let Some(open) =
            self.prs.lock().expect("not poisoned").get(run).cloned()
        else {
            return Ok(false);
        };
        if !open.keep_pushing {
            return Ok(false);
        }
        let slot = self.slot_of_run(run)?;
        let project = slot.project()?;
        let replayed =
            self.replayed(run, &project, Some((&open.head, &open.change)))?;
        let (Some(head), Some(last)) =
            (replayed.copies.last(), replayed.changes.last())
        else {
            return Ok(false);
        };
        let (url, token) = self.github_remote(&slot.name)?;
        let remote = tau_vcs::Remote {
            url: &url,
            token: Some(&token),
        };
        project.blocking().push_branch(
            remote,
            &open.branch,
            Some(&open.head),
            head,
        )?;
        if let Some(open) = self.prs.lock().expect("not poisoned").get_mut(run)
        {
            open.head = head.clone();
            open.change = last.change_id.clone();
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
    pub fn pull_request_checks(
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
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        self.runtime
            .block_on(self.github.checks(&token.token, &open.repo, &open.head))
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
                .spawn_blocking(move || checker.pull_request_checks(&run))
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
