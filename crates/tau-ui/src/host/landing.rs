//! Landing a run's work on its parent, and the branches a run keeps.

use super::*;

/// What [`Host::land`] and [`Host::preview_landing`] work with.
pub(super) struct LandingPlan {
    pub(super) parent: RunId,
    pub(super) project: Project,
    pub(super) parent_workspace: String,
    pub(super) child_workspace: String,
    /// The child's newest commit, from its bookmark.
    pub(super) child_head: String,
    /// The parent's workspace, where the landing runs.
    pub(super) parent_vcs: tau_vcs::Vcs,
}

/// The latest link in `entries`, up to `seq` when given.
pub(super) fn last_link(entries: &[(i64, String)], seq: Option<i64>) -> Option<Link> {
    entries
        .iter()
        .filter(|(at, _)| seq.is_none_or(|seq| *at <= seq))
        .filter_map(|(_, body)| Link::parse(body))
        .next_back()
}

/// See [`Host::branch_code`].
pub(super) async fn branch_code(
    store: Store,
    slot: anyhow::Result<RepoSlot>,
    main: RunId,
    fork: RunId,
) -> anyhow::Result<BranchCode> {
    let slot = slot?;
    let project = tokio::task::spawn_blocking(move || slot.project()).await??;
    let record = store
        .run(&fork.0)
        .await?
        .ok_or_else(|| anyhow::anyhow!("No run {}", fork.0))?;
    let RunKind::Fork { parent, fork_seq } = record.kind else {
        anyhow::bail!("{} is not a fork", fork.0);
    };
    let base = last_link(
        &store.plugin_entries(&parent, WORKSPACE_PLUGIN).await?,
        Some(fork_seq),
    )
    .ok_or_else(|| anyhow::anyhow!("The fork point has no commit"))?;
    let head = |run: &RunId| {
        let store = store.clone();
        let run = run.0.to_string();
        let base = base.clone();
        async move {
            let entries = store.plugin_entries(&run, WORKSPACE_PLUGIN).await?;
            anyhow::Ok(last_link(&entries, None).unwrap_or(base))
        }
    };
    let main_head = head(&main).await?;
    let fork_head = head(&fork).await?;
    tokio::task::spawn_blocking(move || {
        // Each at the commit its change has now.
        let [base, main_head, fork_head]: [Link; 3] = project
            .current([base, main_head, fork_head])?
            .try_into()
            .map_err(|_| anyhow::anyhow!("Three links went in"))?;
        let (base, main_head, fork_head) =
            (base.commit_id, main_head.commit_id, fork_head.commit_id);
        let stats = |from: &str, to: &str| -> anyhow::Result<Vec<FileStat>> {
            Ok(project.diff(from, to)?.iter().map(file_stat).collect())
        };
        Ok(BranchCode {
            main: stats(&base, &main_head)?,
            fork: stats(&base, &fork_head)?,
            between: project
                .diff(&main_head, &fork_head)?
                .iter()
                .map(|file| FileChange {
                    stat: file_stat(file),
                    lines: tau_ui_kit::diff::parse(&file.text),
                })
                .collect(),
        })
    })
    .await?
}

pub(super) fn file_stat(file: &FileDiff) -> FileStat {
    FileStat {
        path: file.path.clone(),
        kind: match file.kind {
            ChangeKind::Added => FileKind::Added,
            ChangeKind::Modified => FileKind::Modified,
            ChangeKind::Removed => FileKind::Removed,
        },
        added: file.added,
        removed: file.removed,
    }
}

impl Host {
    /// What landing `child` on its parent would do (ADR 0009): its
    /// changes as they would sit on the parent's stack, and the files
    /// that would conflict. Changes nothing.
    pub fn preview_landing(&self, child: &RunId) -> anyhow::Result<Landing> {
        let landing = self.landing(child)?;
        let into = self.bookmark_of(&landing.parent, &landing.project)?;
        Ok(self.runtime.block_on(landing.parent_vcs.land(
            &landing.child_head,
            into,
            false,
        ))?)
    }

    /// Lands `child` on its parent (ADR 0009): restacks its changes onto
    /// the parent's newest commit, records them as links in the parent,
    /// and closes the child: its workspace goes, and so does its
    /// bookmark. Both runs must be idle.
    pub fn land(&self, child: &RunId) -> anyhow::Result<Landing> {
        let plan = self.landing(child)?;
        let into = self.bookmark_of(&plan.parent, &plan.project)?;
        let landing = self.runtime.block_on(plan.parent_vcs.land(
            &plan.child_head,
            into,
            true,
        ))?;
        // The landed changes join the parent's links, oldest first, at
        // the parent's latest turn, so forks, the compare view and pull
        // requests see them as the parent's own.
        let turn = self
            .link(&plan.parent, None)?
            .map_or(0, |(_, link)| link.turn);
        let entries = landing
            .changes
            .iter()
            .rev()
            .map(|change| {
                let link = Link {
                    turn,
                    workspace: plan.parent_workspace.clone(),
                    commit_id: change.commit_id.clone(),
                    change_id: change.change_id.clone(),
                    changed: true,
                    from: Some(child.0.to_string()),
                    snapshot: false,
                };
                Ok(Entry::Plugin {
                    plugin: WORKSPACE_PLUGIN.to_owned(),
                    body: serde_json::to_string(&link)?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        // And a record to draw the landing's card from, in history.
        let record = LandingRecord {
            from: child.0.to_string(),
            title: self.title_of(child)?,
            landing: landing.clone(),
        };
        let mut entries = entries;
        entries.push(Entry::Plugin {
            plugin: LANDING_RECORD.to_owned(),
            body: serde_json::to_string(&record)?,
        });
        self.runtime.block_on(self.store.append_turn(
            &plan.parent.0,
            &entries,
            TurnUsage::default(),
        ))?;
        // The child's changes live on the parent's stack now.
        plan.project.forget_workspace(&plan.child_workspace)?;
        plan.project.remove_bookmark(&bookmark(child))?;
        self.workspaces.lock().expect("not poisoned").remove(child);
        Ok(landing)
    }

    /// Starts `run`'s next turn itself, with `prompt`: the turn that
    /// resolves a landing's conflicts (ADR 0014), on the model the run
    /// was on.
    pub fn start_resolving(
        &self,
        run: &RunId,
        prompt: &str,
    ) -> anyhow::Result<()> {
        let choice = self
            .choices
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned()
            .unwrap_or_else(|| {
                ModelChoice::new(self.config.default_model(), Effort::Auto)
            });
        self.resume(run, prompt, &choice)
    }

    /// Drops `child`: abandons its own changes, the ones its parent does
    /// not have, and closes it like a landing does (ADR 0009). Both runs
    /// must be idle. The operation log keeps what was abandoned.
    pub fn drop_child(&self, child: &RunId) -> anyhow::Result<()> {
        let parent = self.parent_of(child)?;
        for run in [child, &parent] {
            if self.is_running(run) {
                anyhow::bail!(
                    "{} is still running; drop it once it stops",
                    run.0
                );
            }
        }
        let project = self.slot_of_run(child)?.project()?;
        // A main chat catches up with trunk first, as for a landing, and
        // keeps what its working copy stands on: its commits that an
        // update moved trunk past are its own, not the child's.
        let main = self.is_main(&parent);
        if main {
            self.catch_up(&project, DEFAULT_WORKSPACE)?;
        }
        if let Some(head) = project.bookmark(&bookmark(child))? {
            let stands_on = match project.workspace_head(DEFAULT_WORKSPACE)? {
                Some(wc) if main => project.parent_of(&wc)?,
                _ => None,
            };
            let keep = match stands_on {
                Some(keep) => keep,
                None => match project
                    .bookmark(&self.bookmark_of(&parent, &project)?)?
                {
                    Some(keep) => keep,
                    None => project.trunk()?,
                },
            };
            project.abandon_between(&keep, &head)?;
        }
        let workspace = self
            .workspaces
            .lock()
            .expect("not poisoned")
            .remove(child)
            .or(self.link(child, None)?.map(|(_, link)| link.workspace));
        if let Some(name) = workspace {
            project.forget_workspace(&name)?;
        }
        project.remove_bookmark(&bookmark(child))?;
        Ok(())
    }

    /// The run `child` was forked from or called by.
    pub(super) fn parent_of(&self, child: &RunId) -> anyhow::Result<RunId> {
        let record = self
            .runtime
            .block_on(self.store.run(&child.0))?
            .ok_or_else(|| anyhow::anyhow!("No run {}", child.0))?;
        match record.kind {
            RunKind::Fork { parent, .. } | RunKind::Subagent { parent, .. } => {
                Ok(RunId(parent.into()))
            }
            RunKind::Root => anyhow::bail!("{} has no parent", child.0),
        }
    }

    /// Everything landing `child` needs, once both runs are idle.
    pub(super) fn landing(&self, child: &RunId) -> anyhow::Result<LandingPlan> {
        let parent = self.parent_of(child)?;
        for run in [child, &parent] {
            self.settle(run);
            if self.is_running(run) {
                anyhow::bail!("{} is still running; land once it stops", run.0);
            }
        }
        let project = self.slot_of_run(child)?.project()?;
        let workspace_of = |run: &RunId| -> anyhow::Result<String> {
            let known = self
                .workspaces
                .lock()
                .expect("not poisoned")
                .get(run)
                .cloned();
            match known {
                Some(name) => Ok(name),
                None => self
                    .link(run, None)?
                    .map(|(_, link)| link.workspace)
                    .ok_or_else(|| {
                        anyhow::anyhow!("{} has not finished a turn", run.0)
                    }),
            }
        };
        let child_workspace = workspace_of(child)?;
        let parent_workspace = match workspace_of(&parent) {
            // A main chat works in the repository's own checkout.
            _ if self.is_main(&parent) => {
                self.workspaces
                    .lock()
                    .expect("not poisoned")
                    .insert(parent.clone(), DEFAULT_WORKSPACE.to_owned());
                DEFAULT_WORKSPACE.to_owned()
            }
            Ok(name) => {
                // Opening a workspace that is gone would make a new one
                // on trunk; landing there would lose the parent's work.
                if !project.workspaces()?.contains(&name) {
                    anyhow::bail!("The parent's workspace is gone");
                }
                name
            }
            Err(error) => return Err(error),
        };
        // A main chat takes landings on trunk as it is now. The catch-up
        // may restack the child, so its head is read after it.
        if self.is_main(&parent) {
            self.catch_up(&project, &parent_workspace)?;
        }
        let child_head =
            project.bookmark(&bookmark(child))?.ok_or_else(|| {
                anyhow::anyhow!("{} has no changes to land", child.0)
            })?;
        let parent_vcs =
            project.add_workspace(&parent_workspace, &project.trunk()?)?;
        Ok(LandingPlan {
            parent,
            project,
            parent_workspace,
            child_workspace,
            child_head,
            parent_vcs,
        })
    }

    /// Keeps `run`, a branch of a fork, and removes the workspaces of the
    /// other branches that are not running: the run it was forked from,
    /// and its other forks. Their commits stay in the project.
    pub fn keep_branch(&self, run: &RunId) -> anyhow::Result<()> {
        let project = self.slot_of_run(run)?.project()?;
        let store = self.store.clone();
        let kept = run.0.to_string();
        let running: Vec<String> = self
            .runs
            .lock()
            .expect("not poisoned")
            .keys()
            .map(|run| run.0.to_string())
            .collect();
        self.runtime.block_on(async move {
            let record = store
                .run(&kept)
                .await?
                .ok_or_else(|| anyhow::anyhow!("No run {kept}"))?;
            let RunKind::Fork { parent, .. } = record.kind else {
                return Ok(());
            };
            let family: Vec<String> = store
                .recent_runs(1000)
                .await?
                .into_iter()
                .filter(|other| {
                    other.id == parent
                        || matches!(&other.kind, RunKind::Fork { parent: p, .. } if *p == parent)
                })
                .map(|other| other.id)
                .filter(|id| *id != kept && !running.contains(id))
                .collect();
            for other in family {
                let names: Vec<String> = store
                    .plugin_entries(&other, WORKSPACE_PLUGIN)
                    .await?
                    .iter()
                    .filter_map(|(_, body)| Link::parse(body))
                    .map(|link| link.workspace)
                    .collect();
                let project = project.clone();
                tokio::task::spawn_blocking(move || {
                    names.iter().try_for_each(|name| project.forget_workspace(name))
                })
                .await??;
            }
            Ok(())
        })
    }

    /// The code of `main` and its fork `fork`: what each changed after
    /// the fork point, and how the fork's code differs from the run's.
    pub fn branch_code(
        &self,
        main: &RunId,
        fork: &RunId,
    ) -> impl std::future::Future<Output = anyhow::Result<BranchCode>> + Send + 'static
    {
        branch_code(
            self.store.clone(),
            self.slot_of_run(main),
            main.clone(),
            fork.clone(),
        )
    }
}
