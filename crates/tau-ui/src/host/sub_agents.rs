//! Sub-agents on the host (ADR 0026): each repository's main chat keeps
//! its [`SubAgents`] between its turns, as they run beside it. One that
//! ends with nobody waiting joins main's landing queue, and tau's turn
//! after the drain reports it.

use tau_ui_remote::queue::{SubAgentEnd, Waiting};
use tau_vcs_host::sub_agents::{Ending, Taken, limit_name};

use super::{landing::Reading, lanes::DrainReport, queue::Lane, *};

/// What tau's turn reporting sub-agents ends with.
const REPORT_END: &str = "This message is from tau, not the person: \
    your sub-agents came back. First tell the person where things stand: \
    what landed, what is left, and what failed. Start more work, new \
    sub-agents included, only when what the person asked for still \
    clearly needs it.";

impl Host {
    /// The sub-agents of `repo`'s main chat, made the first time.
    pub(super) fn sub_agents_of(&self, repo: &str) -> tau_vcs_host::SubAgents {
        self.sub_agents
            .lock()
            .expect("not poisoned")
            .entry(repo.to_owned())
            .or_insert_with(|| {
                tau_vcs_host::SubAgents::new(Some(self.events.clone()), None)
            })
            .clone()
    }

    /// Every main chat's sub-agents.
    fn all_sub_agents(&self) -> Vec<tau_vcs_host::SubAgents> {
        let agents = self.sub_agents.lock().expect("not poisoned");
        agents.values().cloned().collect()
    }

    /// Whether `run` is a sub-agent of a main chat this session.
    pub fn is_sub_agent(&self, run: &RunId) -> bool {
        self.all_sub_agents()
            .iter()
            .any(|agents| agents.taken(run).is_some())
    }

    /// Whether `run` is a sub-agent still running.
    pub(super) fn sub_agent_running(&self, run: &RunId) -> bool {
        self.all_sub_agents()
            .iter()
            .any(|agents| agents.is_running(run))
    }

    /// How to steer `run`, a sub-agent still running.
    pub(super) fn sub_agent_control(
        &self,
        run: &RunId,
    ) -> Option<tau_agent::agent::RunControl> {
        self.all_sub_agents()
            .iter()
            .find_map(|agents| agents.control(run))
    }

    /// Stops `run`, a sub-agent still running: its changes are dropped,
    /// and nothing reports it. Returns whether it was one.
    pub(super) fn stop_sub_agent(&self, run: &RunId) -> bool {
        self.all_sub_agents().iter().any(|agents| agents.stop(run))
    }

    /// Whether `wait` took `child` already: it landed, or was reported.
    pub(super) fn sub_agent_taken(&self, child: &RunId) -> bool {
        self.all_sub_agents()
            .iter()
            .any(|agents| agents.taken(child) == Some(true))
    }

    /// Takes `child` to land it from main's queue. `false` when `wait`
    /// took it first.
    pub(super) fn take_sub_agent(&self, child: &RunId) -> bool {
        self.all_sub_agents().iter().all(|agents| {
            !matches!(agents.take(child), Taken::Before(_) | Taken::Running)
        })
    }

    /// `child`, a sub-agent of a main chat, ended: once its work is
    /// checked, it joins main's queue (unless the person stopped it),
    /// and what may land now lands.
    pub async fn sub_agent_ended(
        &self,
        child: &RunId,
    ) -> anyhow::Result<DrainReport> {
        let main = self.parent_of(child).await?;
        let repo = self.slot_of_run(&main).await?.name;
        let agents = self.sub_agents_of(&repo);
        let ending = agents.ended(child).await;
        let state = self.repo_state(&repo);
        let _draining = state.draining.lock().await;
        let end = match ending {
            // Not this session's, or the person stopped it: nothing to
            // land or report.
            None | Some(Ending::Stopped) => None,
            Some(Ending::Done { limit, .. }) => Some(SubAgentEnd {
                limit: limit.map(|limit| limit_name(limit).to_owned()),
                failed: None,
            }),
            Some(
                ending @ (Ending::Failed { .. } | Ending::Retained { .. }),
            ) => Some(SubAgentEnd {
                limit: None,
                failed: Some(ending_text(&ending)),
            }),
        };
        // `wait` took it: its call says what it did.
        if let Some(end) = end
            && agents.taken(child) != Some(true)
        {
            let title = self.title_of(child).await?;
            // What it brings, so its place in the queue shows it before
            // its turn to land previews it again. Read without writing:
            // main may be in a turn. One with nothing to land, or that
            // cannot be read, shows none.
            let (changes, conflicts) = match end.failed {
                Some(_) => (0, Vec::new()),
                None => self
                    .land_dry(child, Reading::Forecast)
                    .await
                    .map(|landing| (landing.changes.len(), landing.conflicts))
                    .unwrap_or_default(),
            };
            let actions = self
                .with_lane(&main, |lane: &mut Lane| {
                    lane.queue(Waiting {
                        run: child.0.to_string(),
                        title,
                        changes,
                        conflicts,
                        confirmed: Vec::new(),
                        sub_agent: Some(end),
                    })
                })
                .await?;
            self.perform(&main, actions).await?;
        }
        let (drained, landings) = self.drain_locked(&main).await?;
        self.report(&main, drained, landings).await
    }

    /// The message of tau's turn after a drain: each sub-agent it
    /// reports, then the conflicts to resolve, if any.
    pub(super) async fn report_prompt(
        &self,
        reported: &[Waiting],
        landings: &[(RunId, Landing)],
        resolve: Option<String>,
    ) -> Option<String> {
        if reported.is_empty() {
            return resolve;
        }
        let mut sections: Vec<String> = Vec::new();
        for waiting in reported {
            let run = RunId(waiting.run.as_str().into());
            let end = waiting.sub_agent.clone().unwrap_or_default();
            if let Some(failed) = &end.failed {
                sections.push(format!(
                    "Sub-agent `{}` ({}) came back with nothing to \
                     land: {failed}.",
                    waiting.title, waiting.run
                ));
                continue;
            }
            let answer = self
                .store
                .run(&run.0)
                .await
                .ok()
                .flatten()
                .and_then(|record| record.result)
                .unwrap_or_default();
            let note = landings
                .iter()
                .find(|(landed, _)| *landed == run)
                .map(|(_, landing)| {
                    tau_vcs_host::sub_agents::landing_note(
                        landing,
                        &landing.conflicts,
                        end.limit.as_deref().and_then(limit_of),
                    )
                })
                .unwrap_or_default();
            sections.push(format!(
                "Sub-agent `{}` ({}) finished.\n\n{answer}\n\n{note}",
                waiting.title, waiting.run
            ));
        }
        sections.extend(resolve);
        sections.push(REPORT_END.to_owned());
        Some(sections.join("\n\n"))
    }
}

/// A limit from its name in the queue's records.
fn limit_of(name: &str) -> Option<tau_agent::event::LimitKind> {
    use tau_agent::event::LimitKind;
    [
        LimitKind::Turns,
        LimitKind::Tokens,
        LimitKind::Usd,
        LimitKind::Time,
    ]
    .into_iter()
    .find(|limit| limit_name(*limit) == name)
}

/// What an ending with nothing to land says, for the report.
fn ending_text(ending: &Ending) -> String {
    match ending {
        Ending::Failed { error } => {
            format!("it failed ({error}), and its changes were dropped")
        }
        Ending::Retained { error, workspace } => format!(
            "it could not finalize its work ({error}); its workspace and \
             bookmark were kept at {} for recovery",
            workspace.display()
        ),
        Ending::Done { .. } => "it finished".to_owned(),
        Ending::Stopped => "the person stopped it".to_owned(),
    }
}
