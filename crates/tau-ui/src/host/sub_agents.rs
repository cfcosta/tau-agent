//! Sub-agents on the host (ADR 0026, 0031): each repository's main chat
//! keeps its [`SubAgents`] between its turns, as they run beside it.
//! Each one that ends joins main's landing queue, and tau's turn after
//! the drain reports it, for main to go on from.

use tau_ui_remote::queue::{SubAgentEnd, Waiting};
use tau_vcs_host::sub_agents::{Ending, Taken, limit_name};

use super::{landing::Reading, lanes::DrainReport, queue::Lane, *};

/// What the person wrote to a sub-agent that it never read, as its main
/// chat got it ([`Host::forward_unread`]).
pub(super) struct Forwarded {
    /// The message main got, quoting what the person wrote.
    pub(super) text: String,
    /// Whether it went into main's turn; else a turn on main starts
    /// with it.
    pub(super) steered: bool,
}

/// What a main chat's runs start with, before the person's message:
/// main leads the work through sub-agents (ADR 0031). It goes in the
/// first message, not the instructions, which every run of the
/// repository shares (ADR 0022).
const ORCHESTRATE: &str = "You are this repository's main chat, and you \
    lead the work as an orchestrator: split what the person asks into \
    tasks that can go on side by side, `spawn` a sub-agent for each, and \
    end your turn while they work. As each one lands, tau starts a turn \
    of yours with what it did: check it against what was asked, resolve \
    any conflicts and commit, then spawn what comes next (follow-ups, \
    fixes, tests) until the request is done. Do yourself only the glue \
    between tasks and work too small to hand off.";

/// Puts [`ORCHESTRATE`] in a main chat's run.
pub(super) struct Orchestrate;

#[async_trait]
impl Plugin for Orchestrate {
    fn name(&self) -> &str {
        "tau-orchestrate"
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        plan.context.push(ORCHESTRATE.to_owned());
        Ok(Box::new(()))
    }
}

/// What tau's turn reporting sub-agents ends with: main goes on
/// orchestrating (ADR 0031).
const REPORT_END: &str = "This message is from tau, not the person: \
    your sub-agents came back. Check what landed against what the person \
    asked, resolve any conflicts and commit, then spawn what comes next \
    (follow-ups, fixes, tests) and end your turn while they work. Tell \
    the person in a line or two what landed, what failed, and what you \
    started. When nothing is left to do, say the request is done.";

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

    /// Whether `run` is a sub-agent whose end went by, while its work
    /// is checked.
    pub(super) fn sub_agent_ending(&self, run: &RunId) -> bool {
        self.all_sub_agents()
            .iter()
            .any(|agents| agents.is_ending(run))
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

    /// Whether main's queue took `child` already, to land it.
    pub(super) fn sub_agent_taken(&self, child: &RunId) -> bool {
        self.all_sub_agents()
            .iter()
            .any(|agents| agents.taken(child) == Some(true))
    }

    /// Takes `child` to land it from main's queue. `false` when it is
    /// still running, or was taken before.
    pub(super) fn take_sub_agent(&self, child: &RunId) -> bool {
        self.all_sub_agents().iter().all(|agents| {
            !matches!(agents.take(child), Taken::Before(_) | Taken::Running)
        })
    }

    /// `child`, a sub-agent of a main chat, ended: once its work is
    /// checked, it joins main's queue (unless the person stopped it),
    /// and what may land now lands. `unread` is what the person wrote to
    /// it that it never read: it goes to main, whose work it is now,
    /// with the report.
    pub async fn sub_agent_ended(
        &self,
        child: &RunId,
        unread: Vec<String>,
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
                unread,
            }),
            Some(
                ending @ (Ending::Failed { .. } | Ending::Retained { .. }),
            ) => Some(SubAgentEnd {
                limit: None,
                failed: Some(ending_text(&ending)),
                unread,
            }),
        };
        if let Some(end) = end {
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

    /// Hands `unread`, which the person wrote to `child` and it never
    /// read, to `main`: steered into main's turn when main is going,
    /// else for a turn on main to start with.
    pub(super) async fn forward_unread(
        &self,
        main: &RunId,
        child: &RunId,
        unread: &[String],
    ) -> anyhow::Result<Forwarded> {
        let title = self.title_of(child).await?;
        let text = forwarded(&title, child, unread);
        let steered = self.steer(main, &text).await? == Delivery::Steered;
        Ok(Forwarded { text, steered })
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
            let unread = unread_note(&end.unread);
            if let Some(failed) = &end.failed {
                sections.push(format!(
                    "Sub-agent `{}` ({}) came back with nothing to \
                     land: {failed}.{unread}",
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
                "Sub-agent `{}` ({}) finished.\n\n{answer}\n\n{note}{unread}",
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

/// What a report adds for the messages the person wrote to a
/// sub-agent that it ended before reading; nothing when there are none.
fn unread_note(unread: &[String]) -> String {
    if unread.is_empty() {
        return String::new();
    }
    format!(
        "\n\nThe person also wrote to it after its last turn, so it never \
         read this. Its work is yours now: take it as written to you.\n\n{}",
        quoted(unread)
    )
}

/// The message main gets for what the person wrote to `child` that it
/// never read.
pub(super) fn forwarded(
    title: &str,
    child: &RunId,
    unread: &[String],
) -> String {
    format!(
        "This message is from tau, not the person: the person wrote to \
         sub-agent `{title}` ({}) after its last turn, so it never read \
         this. Its work is yours now: take it as written to you.\n\n{}",
        child.0,
        quoted(unread)
    )
}

/// Each message as a quote, one after the other.
fn quoted(messages: &[String]) -> String {
    messages
        .iter()
        .map(|message| {
            message
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
