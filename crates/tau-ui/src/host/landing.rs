//! Landing a run's work on its parent, and the branches a run keeps.

use super::{queue::Preview, *};

/// The plugin name of the intent a landing stores in its parent before
/// it changes anything ([`Host::land`]).
pub const LANDING_INTENT: &str = "landing-intent";

/// What a landing is about to do, stored first, so the next start can
/// finish a landing cut off by tau closing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Intent {
    /// The child landing.
    pub(super) from: String,
    /// Its head as the landing read it: the restack's operation records
    /// what it did under it (`Project::landed`).
    pub(super) child_head: String,
    pub(super) child_workspace: String,
    pub(super) parent_workspace: String,
    /// The restack failed: the landing did not happen, and the next
    /// start leaves it be.
    #[serde(default)]
    pub(super) cancelled: bool,
}

/// A step of [`Host::land`] after which tau may close, for tests that
/// cut a landing off there ([`Host::cut_landing_after`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingStep {
    /// The intent is stored.
    Intent,
    /// The restack is done: the child's changes are on the parent.
    Restack,
    /// The links and the landing's record are stored.
    Record,
    /// The child's workspace is forgotten.
    Workspace,
}

impl LandingStep {
    pub const ALL: [Self; 4] =
        [Self::Intent, Self::Restack, Self::Record, Self::Workspace];
}

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
pub(super) fn last_link(
    entries: &[(i64, String)],
    seq: Option<i64>,
) -> Option<Link> {
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
    ///
    /// It works while the parent runs too: then it reads the parent as
    /// it is, without catching a main chat up with trunk. What it finds
    /// is what the person confirms when they land the child
    /// ([`Host::queue_landing`]).
    pub fn preview_landing(&self, child: &RunId) -> anyhow::Result<Landing> {
        let plan = self.landing(child, true)?;
        let into = self.bookmark_of(&plan.parent, &plan.project)?;
        let landing = self.runtime.block_on(plan.parent_vcs.land(
            &plan.child_head,
            into,
            false,
        ))?;
        self.previews.lock().expect("not poisoned").insert(
            child.clone(),
            Preview {
                changes: landing.changes.len(),
                conflicts: landing.conflicts.clone(),
            },
        );
        Ok(landing)
    }

    /// Lands `child` on its parent (ADR 0009): restacks its changes onto
    /// the parent's newest commit, records them as links in the parent,
    /// and closes the child: its workspace goes, and so does its
    /// bookmark. Both runs must be idle.
    ///
    /// Each step can be cut off by tau closing, and the next start
    /// finishes it ([`Host::finish_landings`]): an intent is stored
    /// first, the restack is one jj operation that records what it did,
    /// and the steps after it find nothing to do when done before.
    pub fn land(&self, child: &RunId) -> anyhow::Result<Landing> {
        self.land_as(child, false)
    }

    /// [`Host::land`]; `recovered` when tau finishes, at start, a
    /// landing it was asked for before it closed.
    fn land_as(
        &self,
        child: &RunId,
        recovered: bool,
    ) -> anyhow::Result<Landing> {
        // No run starts while it lands: a main chat's turn would start
        // on the stack the landing rewrites.
        let _starting = self.starting.lock().expect("not poisoned");
        let plan = self.landing(child, false)?;
        let into = self.bookmark_of(&plan.parent, &plan.project)?;
        let intent = Intent {
            from: child.0.to_string(),
            child_head: plan.child_head.clone(),
            child_workspace: plan.child_workspace.clone(),
            parent_workspace: plan.parent_workspace.clone(),
            cancelled: false,
        };
        self.store_intent(&plan.parent, &intent)?;
        self.cut(LandingStep::Intent)?;
        let landed = self.runtime.block_on(plan.parent_vcs.land(
            &plan.child_head,
            into,
            true,
        ));
        let landing = match landed {
            Ok(landing) => landing,
            // It did not land: the next start does not try again.
            Err(error) => {
                let cancelled = Intent {
                    cancelled: true,
                    ..intent
                };
                self.store_intent(&plan.parent, &cancelled)?;
                return Err(error.into());
            }
        };
        self.cut(LandingStep::Restack)?;
        self.finish_landing(
            &plan.project,
            &plan.parent,
            &intent,
            &landing,
            recovered,
        )?;
        Ok(landing)
    }

    /// What a landing does after its restack, each step finding nothing
    /// to do when it was done before: records the landed changes as the
    /// parent's links with a record of the landing, then forgets the
    /// child's workspace and removes its bookmark.
    fn finish_landing(
        &self,
        project: &Project,
        parent: &RunId,
        intent: &Intent,
        landing: &Landing,
        recovered: bool,
    ) -> anyhow::Result<LandingRecord> {
        let child = RunId(intent.from.as_str().into());
        let record = LandingRecord {
            from: intent.from.clone(),
            title: self.title_of(&child)?,
            landing: landing.clone(),
            recovered,
        };
        if self.landing_record(parent, &child)?.is_none() {
            // The landed changes join the parent's links, oldest first,
            // at the parent's latest turn, so forks, the compare view and
            // pull requests see them as the parent's own.
            let turn =
                self.link(parent, None)?.map_or(0, |(_, link)| link.turn);
            let mut entries = landing
                .changes
                .iter()
                .rev()
                .map(|change| {
                    let link = Link {
                        turn,
                        workspace: intent.parent_workspace.clone(),
                        commit_id: change.commit_id.clone(),
                        change_id: change.change_id.clone(),
                        changed: true,
                        from: Some(intent.from.clone()),
                        snapshot: false,
                    };
                    Ok(Entry::Plugin {
                        plugin: WORKSPACE_PLUGIN.to_owned(),
                        body: serde_json::to_string(&link)?,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            // And a record to draw the landing's card from, in history,
            // in the same write.
            entries.push(Entry::Plugin {
                plugin: LANDING_RECORD.to_owned(),
                body: serde_json::to_string(&record)?,
            });
            self.runtime.block_on(self.store.append_turn(
                &parent.0,
                &entries,
                TurnUsage::default(),
            ))?;
        }
        self.cut(LandingStep::Record)?;
        // The child's changes live on the parent's stack now.
        project.forget_workspace(&intent.child_workspace)?;
        self.cut(LandingStep::Workspace)?;
        project.remove_bookmark(&bookmark(&child))?;
        self.session(&child, |run| run.workspace.take());
        Ok(record)
    }

    /// The record `parent` keeps of `child`'s landing, once stored.
    fn landing_record(
        &self,
        parent: &RunId,
        child: &RunId,
    ) -> anyhow::Result<Option<LandingRecord>> {
        Ok(self
            .runtime
            .block_on(self.store.plugin_entries(&parent.0, LANDING_RECORD))?
            .iter()
            .filter_map(|(_, body)| {
                serde_json::from_str::<LandingRecord>(body).ok()
            })
            .find(|record| *record.from == *child.0))
    }

    fn store_intent(
        &self,
        parent: &RunId,
        intent: &Intent,
    ) -> anyhow::Result<()> {
        self.runtime.block_on(self.store.append_turn(
            &parent.0,
            &[Entry::Plugin {
                plugin: LANDING_INTENT.to_owned(),
                body: serde_json::to_string(intent)?,
            }],
            TurnUsage::default(),
        ))?;
        Ok(())
    }

    /// Stops a landing after `step`, as tau closing there would, when a
    /// test asked for it ([`Host::cut_landing_after`]).
    fn cut(&self, step: LandingStep) -> anyhow::Result<()> {
        if *self.cut_landing.lock().expect("not poisoned") == Some(step) {
            anyhow::bail!("tau closed after the landing's {step:?} step");
        }
        Ok(())
    }

    /// Makes every landing stop after `step`, as if tau closed there:
    /// for tests of what the next start finishes. `None` lets landings
    /// run whole.
    pub fn cut_landing_after(&self, step: Option<LandingStep>) {
        *self.cut_landing.lock().expect("not poisoned") = step;
    }

    /// Finishes the landings tau was in the middle of when it closed:
    /// for each stored intent with no record of its landing, a restack
    /// the project's operations show done is recorded and the child
    /// closed; one not done yet lands now. Returns the records of the
    /// landings it finished, each marked recovered, for their cards. A
    /// landing whose record was stored is done but for its workspace
    /// and bookmark, which the sweep after takes.
    pub fn finish_landings(&self) -> anyhow::Result<Vec<LandingRecord>> {
        let intents = self
            .runtime
            .block_on(self.store.plugin_entries_everywhere(LANDING_INTENT))?;
        // The latest intent of each child, with the parent it is on.
        let mut latest: Vec<(RunId, Intent)> = Vec::new();
        for (parent, body) in intents {
            let Ok(intent) = serde_json::from_str::<Intent>(&body) else {
                continue;
            };
            latest.retain(|(_, known)| known.from != intent.from);
            latest.push((RunId(parent.into()), intent));
        }
        let mut finished = Vec::new();
        for (parent, intent) in latest {
            let child = RunId(intent.from.as_str().into());
            if intent.cancelled || self.ending_of(&child)?.is_some() {
                continue;
            }
            let finish = || -> anyhow::Result<LandingRecord> {
                let project = self.slot_of_run(&child)?.project()?;
                match project.landed(&intent.child_head)? {
                    Some(landing) => self.finish_landing(
                        &project, &parent, &intent, &landing, true,
                    ),
                    None => {
                        self.land_as(&child, true)?;
                        self.landing_record(&parent, &child)?.ok_or_else(|| {
                            anyhow::anyhow!("The landing left no record")
                        })
                    }
                }
            };
            match finish() {
                Ok(record) => finished.push(record),
                Err(error) => eprintln!(
                    "tau-ui: cannot finish the landing of {}: {error:#}",
                    child.0
                ),
            }
        }
        Ok(finished)
    }

    /// Starts `run`'s next turn itself, with `prompt`: the turn that
    /// resolves a landing's conflicts (ADR 0014), on the model the run
    /// was on.
    pub fn start_resolving(
        &self,
        run: &RunId,
        prompt: &str,
    ) -> anyhow::Result<()> {
        let choice = self.session_of(run).choice.unwrap_or_else(|| {
            ModelChoice::new(self.config.default_model(), Effort::Auto)
        });
        self.resume_as(run, prompt, &choice, self.is_main(run))
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
        // The record first: once it is stored the chat takes no more
        // messages, and should tau close before the rest is done, its
        // start finishes it (`Host::sweep`).
        let ending = self.ending_of(child)?;
        if let Some(Ending::Landed { .. }) = ending {
            anyhow::bail!("{} landed already; it cannot be dropped", child.0);
        }
        if ending.is_none() {
            self.runtime.block_on(self.store.append_turn(
                &child.0,
                &[Entry::Plugin {
                    plugin: DROPPED_RECORD.to_owned(),
                    body: serde_json::to_string(&serde_json::json!({
                        "parent": parent.0.to_string(),
                    }))?,
                }],
                TurnUsage::default(),
            ))?;
        }
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
            .session(child, |run| run.workspace.take())
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

    /// Everything landing `child` needs, once both runs are idle; for a
    /// `preview`, the parent may be running.
    pub(super) fn landing(
        &self,
        child: &RunId,
        preview: bool,
    ) -> anyhow::Result<LandingPlan> {
        let parent = self.parent_of(child)?;
        match self.ending_of(child)? {
            None => {}
            Some(Ending::Landed { .. }) => {
                anyhow::bail!("{} landed already", child.0)
            }
            Some(Ending::Dropped) => {
                anyhow::bail!("{} was dropped; it cannot land", child.0)
            }
        }
        for run in [child, &parent] {
            self.settle(run);
        }
        // A preview reads a running parent as it is.
        let parent_busy = self.is_running(&parent);
        for run in [Some(child), (!preview).then_some(&parent)]
            .into_iter()
            .flatten()
        {
            if self.is_running(run) {
                anyhow::bail!("{} is still running; land once it stops", run.0);
            }
        }
        let project = self.slot_of_run(child)?.project()?;
        let workspace_of = |run: &RunId| -> anyhow::Result<String> {
            let known = self.session_of(run).workspace;
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
                self.session(&parent, |run| {
                    run.workspace = Some(DEFAULT_WORKSPACE.to_owned())
                });
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
        if self.is_main(&parent) && !parent_busy {
            self.catch_up(&project, &parent_workspace)?;
        }
        // A completed run may have failed its final commit. Do not land
        // only its earlier commits and then delete the remaining edits.
        // Open the existing workspace, never recreate a missing one.
        let child_vcs = tau_vcs::Vcs::open(
            project.workspace_dir(&child_workspace),
            identity(),
        )?;
        let copy = self.runtime.block_on(child_vcs.working_copy())?;
        if !copy.is_committed() {
            anyhow::bail!(
                "The child has uncommitted or oversized untracked files; its workspace \
                 is retained. Commit or recover its work before landing."
            );
        }
        let child_head =
            project.bookmark(&bookmark(child))?.ok_or_else(|| {
                anyhow::anyhow!("{} has no changes to land", child.0)
            })?;
        let parent_vcs = if parent_busy {
            tau_vcs::Vcs::open(
                project.workspace_dir(&parent_workspace),
                identity(),
            )?
        } else {
            project.add_workspace(&parent_workspace, &project.trunk()?)?
        };
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
