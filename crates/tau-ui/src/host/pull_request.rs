//! Opening a run's work as a pull request, and pushing later commits to it.

use super::*;

/// A pull request opened from a run, as far as tau pushed it.
#[derive(Debug, Clone)]
pub(super) struct OpenPr {
    pub(super) repo: String,
    pub(super) branch: String,
    pub(super) title: String,
    pub(super) keep_pushing: bool,
    /// The branch's commit on GitHub, and the change id of the run's
    /// last commit in it.
    pub(super) head: String,
    pub(super) change: String,
}

/// What the host needs to push a run's commits.
pub(super) struct Pushing {
    pub(super) project: Project,
    pub(super) repo: String,
    pub(super) token: String,
    /// The run's commits, oldest first, after the ones pushed already.
    pub(super) changes: Vec<tau_vcs::StackChange>,
    /// The local commit the first of them builds on, and its commit on
    /// GitHub.
    pub(super) local_parent: String,
    pub(super) remote_parent: String,
    /// The pull request's title, for commit messages.
    pub(super) title: String,
}

impl Host {
    /// The commits on `run`'s stack, oldest first: the model's, and
    /// those its children landed (ADR 0014).
    pub(super) fn stack(
        &self,
        run: &RunId,
        project: &Project,
    ) -> anyhow::Result<Vec<tau_vcs::StackChange>> {
        let Some(head) = project.bookmark(&bookmark(run))? else {
            return Ok(Vec::new());
        };
        Ok(project.stack(&head)?)
    }

    /// Writes a pull request draft from `run`: its changed turns as
    /// commits on its repository's default branch, its prompt as the
    /// title and its last answer as the description.
    pub fn prepare_pull_request(
        &self,
        run: &RunId,
    ) -> anyhow::Result<PullRequest> {
        let slot = self.slot_of_run(run)?;
        let repo = self.github_of(&slot.name).ok_or_else(|| {
            anyhow::anyhow!(
                "Pull requests need a repository cloned from GitHub; {} was \
                 not",
                slot.name
            )
        })?;
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        let project = slot
            .project
            .wait()
            .ok_or_else(|| anyhow::anyhow!("{} has no project", slot.name))?;
        let changes = self.stack(run, &project)?;
        let (Some(first), Some(last)) = (changes.first(), changes.last())
        else {
            anyhow::bail!(
                "The run has no commits, so there is nothing to propose"
            );
        };
        let base = project.parent_of(&first.commit_id)?.ok_or_else(|| {
            anyhow::anyhow!("The run's first commit has no parent")
        })?;
        // Whether the default branch moved on under the run, touching
        // what the run touched.
        let _ = project.update(tau_vcs::UpdateFrom::Remote {
            url: &self.github.clone_url(&repo),
            token: Some(&token.token),
        });
        let trunk = project.trunk()?;
        let changed: Vec<String> = project
            .diff(&base, &last.commit_id)?
            .into_iter()
            .map(|file| file.path)
            .collect();
        let mergeable = trunk == base
            || project
                .diff(&base, &trunk)?
                .iter()
                .all(|file| !changed.contains(&file.path));
        let mut commits = Vec::new();
        let mut previous = base.clone();
        for change in &changes {
            let files = project.diff(&previous, &change.commit_id)?;
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
            previous = change.commit_id.clone();
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
            base: project.default_branch().unwrap_or_else(|| "main".into()),
            mergeable,
            summary: format!(
                "{} changed {} files over {turns} turns.",
                self.title_of(run)?,
                changed.len()
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

    /// Pushes the run's changed turns to the draft's branch and opens
    /// the pull request, asking `reviewers` to review it.
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
        let pushing = self.pushing(run, &draft.repo, title, None)?;
        let (head, change) =
            self.runtime
                .block_on(push(&self.github, &pushing, &draft.head))?;
        let token = pushing.token.clone();
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
                title: title.to_owned(),
                keep_pushing,
                head: head.clone(),
                change,
            },
        );
        Ok((opened, head))
    }

    /// What pushing `run` needs, after `from` (a commit on GitHub and
    /// the change id of the run's last commit in it), or from its base.
    pub(super) fn pushing(
        &self,
        run: &RunId,
        repo: &str,
        title: &str,
        from: Option<(&str, &str)>,
    ) -> anyhow::Result<Pushing> {
        let slot = self.slot_of_run(run)?;
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        let project = slot.project()?;
        let all = self.stack(run, &project)?;
        let first = all
            .first()
            .ok_or_else(|| anyhow::anyhow!("The run has no commits"))?;
        let base = project.parent_of(&first.commit_id)?.ok_or_else(|| {
            anyhow::anyhow!("The run's first commit has no parent")
        })?;
        let (local_parent, remote_parent, changes) = match from {
            None => (base.clone(), base, all),
            Some((remote, pushed)) => {
                // Changes keep their ids when a landing restacks them.
                let at =
                    all.iter().position(|change| change.change_id == pushed);
                let local = at.map_or(base, |at| all[at].commit_id.clone());
                let later = at.map_or(all.clone(), |at| all[at + 1..].to_vec());
                (local, remote.to_owned(), later)
            }
        };
        let title = title.to_owned();
        Ok(Pushing {
            project,
            repo: repo.to_owned(),
            token: token.token,
            changes,
            local_parent,
            remote_parent,
            title,
        })
    }

    /// Pushes the commits `run` made since its pull request's last
    /// push, if it has one that keeps pushing. Returns whether it pushed.
    pub fn push_later_commits(&self, run: &RunId) -> anyhow::Result<bool> {
        let Some(open) =
            self.prs.lock().expect("not poisoned").get(run).cloned()
        else {
            return Ok(false);
        };
        if !open.keep_pushing {
            return Ok(false);
        }
        let pushing = self.pushing(
            run,
            &open.repo,
            &open.title,
            Some((&open.head, &open.change)),
        )?;
        if pushing.changes.is_empty() {
            return Ok(false);
        }
        let (head, change) = self.runtime.block_on(push(
            &self.github,
            &pushing,
            &open.branch,
        ))?;
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

/// Makes a commit on GitHub for each turn in `pushing`, with the files
/// that turn changed, and points `branch` at the last. Returns the
/// branch's new commit and the run's last turn in it.
pub(super) async fn push(
    api: &github::Api,
    pushing: &Pushing,
    branch: &str,
) -> anyhow::Result<(String, String)> {
    let (token, repo) = (&pushing.token, &pushing.repo);
    let mut remote = pushing.remote_parent.clone();
    let mut tree = api
        .commit_tree(token, repo, &remote)
        .await
        .map_err(anyhow::Error::msg)?;
    let mut local = pushing.local_parent.clone();
    let mut pushed = String::new();
    for change in &pushing.changes {
        let mut files = Vec::new();
        for file in pushing.project.diff(&local, &change.commit_id)? {
            let blob =
                match pushing.project.file_at(&change.commit_id, &file.path)? {
                    Some((content, executable)) => Some((
                        api.create_blob(token, repo, &content)
                            .await
                            .map_err(anyhow::Error::msg)?,
                        executable,
                    )),
                    None => None,
                };
            files.push(github::TreeFile {
                path: file.path,
                executable: blob
                    .as_ref()
                    .is_some_and(|(_, executable)| *executable),
                blob: blob.map(|(sha, _)| sha),
            });
        }
        tree = api
            .create_tree(token, repo, &tree, &files)
            .await
            .map_err(anyhow::Error::msg)?;
        // The model's own message (ADR 0014).
        let message = match change.description.trim() {
            "" => pushing.title.clone(),
            described => described.to_owned(),
        };
        remote = api
            .create_commit(token, repo, &message, &tree, &remote)
            .await
            .map_err(anyhow::Error::msg)?;
        local = change.commit_id.clone();
        pushed = change.change_id.clone();
    }
    api.set_branch(token, repo, branch, &remote)
        .await
        .map_err(anyhow::Error::msg)?;
    Ok((remote, pushed))
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
