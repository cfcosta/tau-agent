//! The landing queue on the host (ADR 0024): each main chat's [`Lane`],
//! restored from its stored records, and the repository it lands on as
//! a [`Main`]. Everything here blocks: call it off the interface's
//! thread.

use tau_ui_remote::queue::{MainConflicts, Waiting, conflicts_remain};

pub use super::queue::QUEUE_PLUGIN;
use super::{
    queue::{Drained, Lane, Main, Preview, Record, Unlandable, drain},
    *,
};

/// What a drain of a main chat's queue did, for the interface.
#[derive(Debug)]
pub struct DrainReport {
    pub main: RunId,
    /// Its repository.
    pub repo: String,
    /// The chats that landed, in order, with what each landing did.
    pub landed: Vec<(RunId, Landing)>,
    /// The chats that could not land, and why: they left the queue.
    pub failed: Vec<(RunId, String)>,
    /// tau's turn on main, with this message: it reports the
    /// sub-agents the drain landed or dropped (ADR 0026), and resolves
    /// what the last landing left.
    pub resolve: Option<String>,
    /// Conflicts newly left on main, to tell the person of.
    pub notify: Option<Vec<String>>,
    /// The queue and the mark as the drain left them.
    pub queue: Vec<Waiting>,
    pub conflicts: Option<MainConflicts>,
}

/// Hears that conflicts are still on a main chat after its turn: the
/// main chat, its repository, and the files. The coordinator wires it
/// to the desktop's notifications.
pub type ConflictsHook =
    Arc<dyn Fn(&RunId, &str, &[String], &mut App) + Send + Sync>;

/// A main chat's repository, as a drain lands on it.
struct OnRepo<'a> {
    host: &'a Host,
    main: RunId,
    landings: Vec<(RunId, Landing)>,
}

impl Main for OnRepo<'_> {
    async fn busy(&self) -> bool {
        self.host.settle(&self.main).await;
        self.host.is_running(&self.main)
    }

    async fn conflicts(&mut self) -> anyhow::Result<Vec<String>> {
        self.host.main_conflicts(&self.main).await
    }

    async fn preview(&mut self, run: &str) -> Result<Preview, Unlandable> {
        let child = RunId(run.into());
        self.unlandable(&child).await?;
        self.host
            .preview_landing(&child)
            .await
            .map(|landing| Preview {
                changes: landing.changes.len(),
                conflicts: landing.conflicts,
            })
            .map_err(|error| self.why(error))
    }

    async fn land(&mut self, run: &str) -> Result<Vec<String>, Unlandable> {
        let child = RunId(run.into());
        self.unlandable(&child).await?;
        // `wait` may take a sub-agent first: it lands it itself.
        if !self.host.take_sub_agent(&child) {
            return Err(Unlandable::Gone);
        }
        let landing = self
            .host
            .land(&child)
            .await
            .map_err(|error| self.why(error))?;
        let conflicts = landing.conflicts.clone();
        self.landings.push((child, landing));
        Ok(conflicts)
    }

    async fn title(&self, run: &str) -> String {
        self.host
            .title_of(&RunId(run.into()))
            .await
            .unwrap_or_else(|_| run.to_owned())
    }

    async fn store(&mut self, record: &Record) -> anyhow::Result<()> {
        self.host.store_queue_record(&self.main, record).await
    }
}

impl OnRepo<'_> {
    /// A chat that ended for good leaves the queue; one that is going
    /// again keeps its place.
    async fn unlandable(&self, child: &RunId) -> Result<(), Unlandable> {
        if self.host.sub_agent_taken(child) {
            return Err(Unlandable::Gone);
        }
        match self.host.ending_of(child).await {
            Ok(Some(_)) => return Err(Unlandable::Gone),
            Ok(None) => {}
            Err(error) => return Err(Unlandable::Failed(format!("{error:#}"))),
        }
        if self.host.is_running(&self.main) {
            return Err(Unlandable::Busy);
        }
        Ok(())
    }

    fn why(&self, error: anyhow::Error) -> Unlandable {
        if self.host.is_running(&self.main) {
            Unlandable::Busy
        } else {
            Unlandable::Failed(format!("{error:#}"))
        }
    }
}

impl Host {
    /// Calls `hook` when conflicts are still on a main chat after its
    /// turn.
    pub fn on_conflicts_on_main(mut self, hook: ConflictsHook) -> Self {
        self.conflicts_hook = Some(hook);
        self
    }

    /// Does `f` with `main`'s lane, restored from the store the first
    /// time.
    pub(super) async fn with_lane<R>(
        &self,
        main: &RunId,
        f: impl FnOnce(&mut Lane) -> R,
    ) -> anyhow::Result<R> {
        let known = self.lanes.lock().expect("not poisoned").contains_key(main);
        if !known {
            let records: Vec<Record> = self
                .store
                .plugin_entries(&main.0, QUEUE_PLUGIN)
                .await?
                .iter()
                .filter_map(|(_, body)| serde_json::from_str(body).ok())
                .collect();
            // Restored once: another call may have restored it meanwhile.
            self.lanes
                .lock()
                .expect("not poisoned")
                .entry(main.clone())
                .or_insert_with(|| Lane::restore(&records));
        }
        let mut lanes = self.lanes.lock().expect("not poisoned");
        Ok(f(lanes.get_mut(main).expect("restored above")))
    }

    async fn store_queue_record(
        &self,
        main: &RunId,
        record: &Record,
    ) -> anyhow::Result<()> {
        self.store
            .append_turn(
                &main.0,
                &[Entry::Plugin {
                    plugin: QUEUE_PLUGIN.to_owned(),
                    body: serde_json::to_string(record)?,
                }],
                TurnUsage::default(),
            )
            .await?;
        Ok(())
    }

    /// Carries out what `main`'s lane asked for outside a drain.
    pub(super) async fn perform(
        &self,
        main: &RunId,
        actions: Vec<super::queue::Action>,
    ) -> anyhow::Result<Drained> {
        let mut drained = Drained::default();
        let mut repo = OnRepo {
            host: self,
            main: main.clone(),
            landings: Vec::new(),
        };
        drained.perform(&mut repo, actions).await?;
        Ok(drained)
    }

    /// The files `main`'s stack holds in conflict, once it caught up
    /// with trunk. `main` must be idle.
    pub async fn main_conflicts(
        &self,
        main: &RunId,
    ) -> anyhow::Result<Vec<String>> {
        let project = self.slot_of_run(main).await?.project().await?;
        self.catch_up(&project, DEFAULT_WORKSPACE).await?;
        let vcs = tau_vcs::Vcs::open(
            project.workspace_dir(DEFAULT_WORKSPACE),
            identity(),
        )
        .await?;
        Ok(vcs.conflicts().await?)
    }

    /// `main`'s landing queue and the conflicts on it, as stored.
    pub async fn landing_queue(
        &self,
        main: &RunId,
    ) -> anyhow::Result<(Vec<Waiting>, Option<MainConflicts>)> {
        self.with_lane(main, |lane| {
            (lane.waiting().to_vec(), lane.conflicts().cloned())
        })
        .await
    }

    /// Why a chat may not fork `main` now, if it may not: main has
    /// conflicts.
    pub(super) async fn refuse_fork_of(
        &self,
        main: &RunId,
    ) -> anyhow::Result<()> {
        if let Some(why) =
            self.with_lane(main, |lane| lane.refuse_chat()).await?
        {
            anyhow::bail!(why);
        }
        Ok(())
    }

    /// Main started a turn: nothing lands until it ends.
    pub(super) async fn main_started(&self, main: &RunId) {
        if let Err(error) = self.with_lane(main, Lane::started).await {
            eprintln!("tau-ui: cannot read the landing queue: {error:#}");
        }
    }

    /// The person asked to land `child`: it joins its main chat's queue,
    /// with the conflicts they saw in its last preview as confirmed,
    /// and what may land now lands. Queued already, it takes the
    /// conflicts its last preview found as confirmed, in its place.
    pub async fn queue_landing(
        &self,
        child: &RunId,
    ) -> anyhow::Result<DrainReport> {
        let _draining = self.draining.lock().await;
        let main = self.parent_of(child).await?;
        if !self.is_main(&main) {
            anyhow::bail!(
                "Only a chat under a repository's main chat lands by queue"
            );
        }
        match self.ending_of(child).await? {
            None => {}
            Some(Ending::Landed { .. }) => {
                anyhow::bail!("{} landed already", child.0)
            }
            Some(Ending::Dropped) => {
                anyhow::bail!("{} was dropped; it cannot land", child.0)
            }
        }
        self.settle(child).await;
        if self.is_running(child) {
            anyhow::bail!(
                "{} is still running; land it once it finishes",
                child.0
            );
        }
        let seen = self
            .previews
            .lock()
            .expect("not poisoned")
            .get(child)
            .cloned()
            .unwrap_or_default();
        // A chat never previewed lands only cleanly: nobody confirmed
        // conflicts for it.
        let title = self.title_of(child).await?;
        let actions = self
            .with_lane(&main, |lane| {
                let known = lane.waiting().iter().find(|w| *w.run == *child.0);
                let waiting = match known {
                    Some(known) => Waiting {
                        confirmed: known.conflicts.clone(),
                        title,
                        ..known.clone()
                    },
                    None => Waiting {
                        run: child.0.to_string(),
                        title,
                        changes: seen.changes,
                        conflicts: seen.conflicts.clone(),
                        confirmed: seen.conflicts,
                        sub_agent: None,
                    },
                };
                lane.queue(waiting)
            })
            .await?;
        self.perform(&main, actions).await?;
        let (drained, landings) = self.drain_locked(&main).await?;
        self.report(&main, drained, landings).await
    }

    /// Takes `child` out of its main chat's queue; what may land now
    /// lands.
    pub async fn unqueue(&self, child: &RunId) -> anyhow::Result<DrainReport> {
        let _draining = self.draining.lock().await;
        let main = self.parent_of(child).await?;
        let actions =
            self.with_lane(&main, |lane| lane.unqueue(&child.0)).await?;
        self.perform(&main, actions).await?;
        let (drained, landings) = self.drain_locked(&main).await?;
        self.report(&main, drained, landings).await
    }

    /// The person will resolve `main`'s conflicts themselves.
    pub async fn dismiss_conflicts(
        &self,
        main: &RunId,
    ) -> anyhow::Result<DrainReport> {
        let _draining = self.draining.lock().await;
        let actions = self.with_lane(main, Lane::dismiss).await?;
        self.perform(main, actions).await?;
        self.report(main, Drained::default(), Vec::new()).await
    }

    /// The message tau's turn resolving `main`'s conflicts starts with
    /// again: the last one, or one naming the files.
    pub async fn resolve_again_prompt(
        &self,
        main: &RunId,
    ) -> anyhow::Result<String> {
        self.with_lane(main, |lane| match lane.conflicts() {
            Some(conflicts) if !conflicts.prompt.is_empty() => {
                Ok(conflicts.prompt.clone())
            }
            Some(conflicts) => Ok(conflicts_remain(&conflicts.files)),
            None => Err(anyhow::anyhow!("main has no conflicts left")),
        })
        .await?
    }

    /// `main`'s turn ended: what it left in conflict marks it, or its
    /// mark goes, and what may land now lands.
    pub async fn main_turn_ended(
        &self,
        main: &RunId,
    ) -> anyhow::Result<DrainReport> {
        let _draining = self.draining.lock().await;
        self.settle(main).await;
        if self.is_running(main) {
            return self.report(main, Drained::default(), Vec::new()).await;
        }
        let files = self.main_conflicts(main).await?;
        let actions = self.with_lane(main, |lane| lane.ended(files)).await?;
        let mut drained = self.perform(main, actions).await?;
        let (more, landings) = self.drain_locked(main).await?;
        drained.landed.extend(more.landed);
        drained.failed.extend(more.failed);
        drained.reported.extend(more.reported);
        drained.resolve = more.resolve;
        drained.notify = drained.notify.or(more.notify);
        self.report(main, drained, landings).await
    }

    /// The person stopped `main`'s turn: what it left in conflict marks
    /// it, or its mark goes, but nothing lands and tau starts no turn on
    /// it. What waits lands after main's next turn, or when a sub-agent
    /// ends.
    pub async fn main_turn_stopped(
        &self,
        main: &RunId,
    ) -> anyhow::Result<DrainReport> {
        let _draining = self.draining.lock().await;
        self.settle(main).await;
        if self.is_running(main) {
            return self.report(main, Drained::default(), Vec::new()).await;
        }
        let files = self.main_conflicts(main).await?;
        let actions = self.with_lane(main, |lane| lane.ended(files)).await?;
        let drained = self.perform(main, actions).await?;
        self.report(main, drained, Vec::new()).await
    }

    /// tau's resolving turn on `main` did not start: main is idle with
    /// what the landing left on it.
    pub async fn resolving_failed(
        &self,
        main: &RunId,
    ) -> anyhow::Result<DrainReport> {
        let _draining = self.draining.lock().await;
        let files = self.main_conflicts(main).await?;
        let actions = self
            .with_lane(main, |lane| lane.not_resolving(files))
            .await?;
        let drained = self.perform(main, actions).await?;
        self.report(main, drained, Vec::new()).await
    }

    /// Lands what may land on `main` now (`queue::drain`).
    pub async fn drain_main(
        &self,
        main: &RunId,
    ) -> anyhow::Result<DrainReport> {
        let _draining = self.draining.lock().await;
        let (drained, landings) = self.drain_locked(main).await?;
        self.report(main, drained, landings).await
    }

    pub(super) async fn drain_locked(
        &self,
        main: &RunId,
    ) -> anyhow::Result<(Drained, Vec<(RunId, Landing)>)> {
        // A copy: the interface reads the lane meanwhile, and what changes
        // it is serialized by `draining`.
        let mut lane = self.with_lane(main, |lane| lane.clone()).await?;
        let mut repo = OnRepo {
            host: self,
            main: main.clone(),
            landings: Vec::new(),
        };
        let drained = drain(&mut lane, &mut repo).await;
        let landings = std::mem::take(&mut repo.landings);
        // The lane goes back whatever the drain came to: what it did is
        // stored as it went.
        self.with_lane(main, |kept| {
            // A turn main started meanwhile still counts.
            if kept.is_busy() {
                lane.started();
            }
            *kept = lane;
        })
        .await?;
        Ok((drained?, landings))
    }

    pub(super) async fn report(
        &self,
        main: &RunId,
        drained: Drained,
        landings: Vec<(RunId, Landing)>,
    ) -> anyhow::Result<DrainReport> {
        let (queue, conflicts) = self.landing_queue(main).await?;
        let resolve = self
            .report_prompt(&drained.reported, &landings, drained.resolve)
            .await;
        Ok(DrainReport {
            main: main.clone(),
            repo: self
                .slot_of_run(main)
                .await
                .map(|slot| slot.name)
                .unwrap_or_default(),
            landed: landings,
            failed: drained
                .failed
                .into_iter()
                .map(|(run, why)| (RunId(run.as_str().into()), why))
                .collect(),
            resolve,
            notify: drained.notify,
            queue,
            conflicts,
        })
    }

    /// Drains every listed repository's main chat, as tau starts: what
    /// waited when it closed lands now, and main's conflicts are read
    /// again.
    pub async fn drain_all(&self) -> Vec<DrainReport> {
        let mut reports = Vec::new();
        for main in self.mains() {
            let main = RunId(main.as_str().into());
            match self.drain_main(&main).await {
                Ok(report) => reports.push(report),
                Err(error) => eprintln!(
                    "tau-ui: cannot land what waits on {}: {error:#}",
                    main.0
                ),
            }
        }
        reports
    }
}

/// Holds a resolving turn's stop once while conflicts remain on main:
/// "Conflicts remain in a.rs, b.rs. Resolve them and commit."
pub(super) struct ResolveHold {
    pub(super) vcs: tau_vcs::Vcs,
}

#[async_trait]
impl Plugin for ResolveHold {
    fn name(&self) -> &str {
        "tau-resolve-hold"
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(HoldOnce {
            vcs: self.vcs.clone(),
            held: false,
        }))
    }
}

struct HoldOnce {
    vcs: tau_vcs::Vcs,
    held: bool,
}

#[async_trait]
impl PluginRun for HoldOnce {
    async fn before_stop(
        &mut self,
        _message: &tau_ai::message::AssistantMessage,
        _ctx: &PluginCtx,
    ) -> Result<tau_agent::plugin::StopDecision, PluginError> {
        use tau_agent::plugin::StopDecision;
        if self.held {
            return Ok(StopDecision::Stop);
        }
        let files = self
            .vcs
            .conflicts()
            .await
            .map_err(|error| PluginError::from(error.to_string()))?;
        if files.is_empty() {
            return Ok(StopDecision::Stop);
        }
        self.held = true;
        Ok(StopDecision::Continue(conflicts_remain(&files)))
    }
}
