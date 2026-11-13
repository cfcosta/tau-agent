//! Sub-agents that run beside their caller (ADR 0009, 0015, 0026,
//! 0031): [`Spawn`] starts one and answers at once, and [`SubAgents`]
//! keeps them between the caller's turns. Nothing waits for one: the
//! host lands each as it ends. Their workspaces are the VCS's
//! ([`RunWorkspace`]).
//!
//! 1. The caller's work must be committed (ADR 0014): `spawn` refuses
//!    while its workspace holds work not committed
//!    ([`RunWorkspace::fork_child`]). The sub-agent's workspace starts
//!    on the caller's newest commit.
//! 2. The sub-agent is a run of its own under the caller, in a chat of
//!    its own. It forks the caller: it sees the conversation, the batch
//!    its call is in, and then its task; a `fresh` one sees its task
//!    alone. It commits its work on its own stack. Up to
//!    [`MAX_RUNNING`] run at once; the caller goes on meanwhile, and
//!    stopping the caller does not stop them.
//! 3. When one ends, its work is checked. One that finished, or that a
//!    limit stopped, can land; one that failed or was stopped has its
//!    changes dropped and closes: its workspace and head go.
//! 4. The host takes each one that ended ([`SubAgents::take`]) and
//!    lands it on the caller when the caller is idle, one landing at a
//!    time. A landing that conflicts lands its conflicts for the caller
//!    to resolve. One spawned with `land: false` never lands: its
//!    commits stay beside the caller's, under its head, for the
//!    caller to read and take files from ([`Ending::Kept`]).

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use statig::{
    Outcome as Next,
    Outcome::{Handled, Super, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};
use tau_agent::{
    agent::{Agent, AgentError, Outcome, Run, RunControl},
    error::ToolError,
    event::{LimitKind, RunEvent, StopReason},
    tool::{AgentTool, ExecutionMode, RunId, ToolCtx, ToolOutput},
};
use tau_ai::responses::request::ReasoningEffort;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

use crate::vcs::{RunWorkspace, SPAWN, SpawnNotes};

/// What a call asks of its sub-agent's model. `None` keeps the
/// caller's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildModel {
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
}

/// Builds a sub-agent's agent around its workspace, on the model its
/// call asks for: the tools and plugins it runs with, the workspace's
/// among them.
pub type ChildAgent = Arc<
    dyn Fn(Arc<dyn RunWorkspace>, &ChildModel) -> ChildFuture + Send + Sync,
>;

/// What [`ChildAgent`] gives back: the sub-agent's agent, once the
/// plugins it runs with are built.
pub type ChildFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Agent, ToolError>> + Send>,
>;

/// Sub-agents of one caller that run at once. `spawn` past it is
/// refused.
pub const MAX_RUNNING: usize = 8;

/// How a sub-agent ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// Its work is committed and can land: it finished, or `limit` cut
    /// it short. `text` is its answer.
    Done {
        text: String,
        limit: Option<LimitKind>,
    },
    /// It finished, or `limit` cut it short, and was spawned not to
    /// land: its commits stay as they are, under its head, and its
    /// workspace closed. `changes` are its own, oldest first, by change
    /// id; none when it committed nothing.
    Kept {
        text: String,
        limit: Option<LimitKind>,
        changes: Vec<String>,
    },
    /// It failed: its changes were dropped.
    Failed { error: String },
    /// The person stopped it: its changes were dropped.
    Stopped,
    /// Its work could not be checked: nothing lands, and its workspace
    /// and head were kept at `workspace` for recovery.
    Retained { error: String, workspace: PathBuf },
}

/// Who gets a sub-agent that ended: [`SubAgents::take`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Taken {
    /// Not one of these sub-agents, as after a restart: whoever asks
    /// may land it.
    Unknown,
    /// It is still running.
    Running,
    /// It is the asker's now, ended as it says.
    Now(Ending),
    /// Someone took it before.
    Before(Ending),
}

/// Hears that a sub-agent ended, after its work was checked.
pub type OnEnd = Arc<dyn Fn(&RunId, &Ending) + Send + Sync>;

/// One caller's sub-agents, between its turns. Clones share them.
#[derive(Clone)]
pub struct SubAgents(Arc<Inner>);

struct Inner {
    children: Mutex<Vec<Child>>,
    /// Bumped whenever a sub-agent ends, for [`SubAgents::ended`].
    changed: watch::Sender<u64>,
    /// Slots for the sub-agents running at once.
    slots: Arc<Semaphore>,
    /// Where the sub-agents' events go; dropped when `None`.
    events: Option<mpsc::UnboundedSender<RunEvent>>,
    on_end: Option<OnEnd>,
}

/// One sub-agent, from its spawn until someone takes its ending.
struct Child {
    run: RunId,
    task: String,
    control: RunControl,
    life: StateMachine<Life>,
}

/// Where a sub-agent is in its life: reading, ending, its work being
/// checked, ended, taken.
#[derive(Default)]
struct Life;

/// What happens to a sub-agent.
enum Event {
    /// The person stopped it.
    Stop,
    /// Its `RunEnd` went by: it reads nothing more.
    RunEnd,
    /// Its run came out: its work is checked now.
    Outcome,
    /// Its work is checked: it ended so, its end closing that workspace.
    Settled(Ending, Option<PathBuf>),
    /// Someone takes it, to land it or report it.
    Take,
}

/// What a [`Life`] answers an [`Event`] with.
#[derive(Debug, Default)]
struct Answer {
    /// Whether a stop was heard.
    stopped: bool,
    /// The run is to be cancelled.
    cancel: bool,
    taken: Option<Taken>,
}

#[state_machine(initial = "State::reading(false)", state(derive(Debug, Clone)))]
impl Life {
    /// Running, and reading what it is steered with; `stop` once the
    /// person stopped it.
    #[state(superstate = "running")]
    fn reading(
        stop: &mut bool,
        context: &mut Answer,
        event: &Event,
    ) -> Next<State> {
        match event {
            Event::Stop => {
                context.cancel = true;
                *stop = true;
                context.stopped = true;
                Handled
            }
            Event::RunEnd => Transition(State::ending(*stop)),
            Event::Outcome => Transition(State::checking(*stop)),
            _ => Super,
        }
    }

    /// Its `RunEnd` went by, its run not out yet: a stop is still in
    /// time to drop its work.
    #[state(superstate = "running")]
    fn ending(
        stop: &mut bool,
        context: &mut Answer,
        event: &Event,
    ) -> Next<State> {
        match event {
            Event::Stop => {
                *stop = true;
                context.stopped = true;
                Handled
            }
            Event::Outcome => Transition(State::checking(*stop)),
            _ => Super,
        }
    }

    /// Its work is being checked, as stopped or not: too late to stop.
    #[state(superstate = "running")]
    fn checking(stopped: &mut bool, event: &Event) -> Next<State> {
        let _ = stopped;
        match event {
            Event::Settled(ending, closed) => {
                Transition(State::ended(ending.clone(), closed.clone()))
            }
            _ => Super,
        }
    }

    /// Still going: nobody can take it yet.
    #[superstate]
    fn running(context: &mut Answer, event: &Event) -> Next<State> {
        if let Event::Take = event {
            context.taken = Some(Taken::Running);
        }
        Handled
    }

    /// It ended so, and nobody took it yet.
    #[state]
    fn ended(
        ending: &mut Ending,
        closed: &mut Option<PathBuf>,
        context: &mut Answer,
        event: &Event,
    ) -> Next<State> {
        match event {
            Event::Take => {
                context.taken = Some(Taken::Now(ending.clone()));
                Transition(State::taken(ending.clone(), closed.clone()))
            }
            _ => Handled,
        }
    }

    /// Someone took it, ended so.
    #[state]
    fn taken(
        ending: &mut Ending,
        closed: &mut Option<PathBuf>,
        context: &mut Answer,
        event: &Event,
    ) -> Next<State> {
        let _ = closed;
        if let Event::Take = event {
            context.taken = Some(Taken::Before(ending.clone()));
        }
        Handled
    }
}

impl State {
    fn is_running(&self) -> bool {
        matches!(
            self,
            Self::Reading { .. } | Self::Ending { .. } | Self::Checking { .. }
        )
    }

    /// How it ended, once its work was checked.
    fn outcome(&self) -> Option<&Ending> {
        match self {
            Self::Ended { ending, .. } | Self::Taken { ending, .. } => {
                Some(ending)
            }
            Self::Reading { .. }
            | Self::Ending { .. }
            | Self::Checking { .. } => None,
        }
    }
}

impl Default for SubAgents {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl SubAgents {
    /// Sub-agents whose events go to `events`, and whose ends `on_end`
    /// hears.
    pub fn new(
        events: Option<mpsc::UnboundedSender<RunEvent>>,
        on_end: Option<OnEnd>,
    ) -> Self {
        Self(Arc::new(Inner {
            children: Mutex::default(),
            changed: watch::Sender::new(0),
            slots: Arc::new(Semaphore::new(MAX_RUNNING)),
            events,
            on_end,
        }))
    }

    fn with<R>(&self, f: impl FnOnce(&mut Vec<Child>) -> R) -> R {
        f(&mut self.0.children.lock().expect("not poisoned"))
    }

    /// Gives `event` to `run`, if it is one of these, and its answer.
    fn tell(&self, run: &RunId, event: &Event) -> Option<Answer> {
        self.with(|children| {
            let child = children.iter_mut().find(|c| c.run == *run)?;
            let mut answer = Answer::default();
            child.life.handle_with_context(event, &mut answer);
            if answer.cancel {
                child.control.cancel();
            }
            Some(answer)
        })
    }

    /// `run`'s state, if it is one of these.
    fn state(&self, run: &RunId) -> Option<State> {
        self.with(|children| {
            children
                .iter()
                .find(|c| c.run == *run)
                .map(|c| c.life.state().clone())
        })
    }

    /// Whether `run` is one of these and still running.
    pub fn is_running(&self, run: &RunId) -> bool {
        self.state(run).is_some_and(|state| state.is_running())
    }

    /// The ones still running.
    pub fn running(&self) -> Vec<RunId> {
        self.with(|children| {
            children
                .iter()
                .filter(|c| c.life.state().is_running())
                .map(|c| c.run.clone())
                .collect()
        })
    }

    /// How to steer `run`, while it runs and still reads: none once
    /// its `RunEnd` went by, as what it was steered with then would be
    /// lost.
    pub fn control(&self, run: &RunId) -> Option<RunControl> {
        self.with(|children| {
            children
                .iter()
                .find(|c| {
                    c.run == *run
                        && matches!(c.life.state(), State::Reading { .. })
                })
                .map(|c| c.control.clone())
        })
    }

    /// Whether `run`'s `RunEnd` went by while its work is still being
    /// checked: it is running for a moment more.
    pub fn is_ending(&self, run: &RunId) -> bool {
        matches!(
            self.state(run),
            Some(State::Ending { .. } | State::Checking { .. })
        )
    }

    /// Stops `run`, if it is running and its work is not being checked
    /// yet: its changes are dropped. Returns whether it was.
    pub fn stop(&self, run: &RunId) -> bool {
        self.tell(run, &Event::Stop)
            .is_some_and(|answer| answer.stopped)
    }

    /// Waits for `run` to end and its work to be checked. `None` when it
    /// is not one of these.
    pub async fn ended(&self, run: &RunId) -> Option<Ending> {
        let mut changed = self.0.changed.subscribe();
        loop {
            if let Some(ending) = self.state(run)?.outcome() {
                return Some(ending.clone());
            }
            if changed.changed().await.is_err() {
                return None;
            }
        }
    }

    /// The directory of `run`'s workspace, if its end closed it: what
    /// it left running there has nowhere to go on.
    pub fn closed(&self, run: &RunId) -> Option<PathBuf> {
        match self.state(run)? {
            State::Ended { closed, .. } | State::Taken { closed, .. } => closed,
            State::Reading { .. }
            | State::Ending { .. }
            | State::Checking { .. } => None,
        }
    }

    /// Takes `run`, once it ended, to land it or report it: one taker
    /// gets it, the others learn it went.
    pub fn take(&self, run: &RunId) -> Taken {
        self.tell(run, &Event::Take)
            .and_then(|answer| answer.taken)
            .unwrap_or(Taken::Unknown)
    }

    /// Whether someone took `run` ([`Self::take`]); `None` when it is
    /// not one of these.
    pub fn taken(&self, run: &RunId) -> Option<bool> {
        self.state(run)
            .map(|state| matches!(state, State::Taken { .. }))
    }

    /// The task `run` was spawned on.
    pub fn task(&self, run: &RunId) -> Option<String> {
        self.with(|children| {
            children
                .iter()
                .find(|c| c.run == *run)
                .map(|c| c.task.clone())
        })
    }

    /// A slot for one more sub-agent, if fewer than [`MAX_RUNNING`]
    /// run.
    fn slot(&self) -> Option<OwnedSemaphorePermit> {
        self.0.slots.clone().try_acquire_owned().ok()
    }

    /// Follows `run`, a sub-agent working in `workspace` from `base` on
    /// `task`: passes its events on, then checks its work as it ends,
    /// for landing or, when `land` is false, to keep beside the caller's.
    fn follow(
        &self,
        mut run: Run,
        task: String,
        workspace: Arc<dyn RunWorkspace>,
        base: String,
        land: bool,
        slot: OwnedSemaphorePermit,
    ) {
        let id = run.id();
        self.with(|children| {
            children.push(Child {
                run: id.clone(),
                task,
                control: run.control(),
                life: Life.state_machine(),
            })
        });
        let agents = self.clone();
        tokio::spawn(async move {
            {
                let mut events = run.events();
                while let Some(event) = events.next().await {
                    // Marked before anyone hears its end, so nobody
                    // steers it after (`Self::control`).
                    if matches!(&event, RunEvent::RunEnd { run, .. } if *run == id)
                    {
                        agents.tell(&id, &Event::RunEnd);
                    }
                    if let Some(sink) = &agents.0.events {
                        let _ = sink.send(event);
                    }
                }
            }
            let outcome = run.outcome().await;
            drop(slot);
            // From here a stop is too late: the work is checked as it
            // stands.
            agents.tell(&id, &Event::Outcome);
            let stopped = matches!(
                agents.state(&id),
                Some(State::Checking { stopped: true })
            );
            let settled =
                settle(outcome, stopped, land, &id, workspace.as_ref(), &base)
                    .await;
            let ending = settled.ending;
            agents.tell(&id, &Event::Settled(ending.clone(), settled.closed));
            agents.0.changed.send_modify(|n| *n += 1);
            if let Some(on_end) = &agents.0.on_end {
                on_end(&id, &ending);
            }
        });
    }
}

/// How a sub-agent came out ([`settle`]), and the directory of the
/// workspace its end closed, if it closed one.
struct Settled {
    ending: Ending,
    closed: Option<PathBuf>,
}

/// How a sub-agent's run came out, once its work is checked: work that
/// can land is committed; work not to land is kept and its workspace
/// closes; a failed or stopped one's changes are dropped and it closes.
async fn settle(
    outcome: Result<Outcome, AgentError>,
    stopped: bool,
    land: bool,
    run: &RunId,
    workspace: &dyn RunWorkspace,
    base: &str,
) -> Settled {
    let ending = match outcome {
        // Stopped before its work was checked: dropped, however its run
        // came out.
        _ if stopped => Ending::Stopped,
        Ok(outcome) => match outcome.stop {
            StopReason::Stop => Ending::Done {
                text: outcome.text,
                limit: None,
            },
            // A sub-agent at a limit committed its work at its end, as
            // any run does (ADR 0014), so it lands like one that
            // finished.
            StopReason::Limit(limit) => Ending::Done {
                text: outcome.text,
                limit: Some(limit),
            },
            StopReason::Cancelled => Ending::Failed {
                error: "it was cancelled".into(),
            },
            StopReason::Error(error) => Ending::Failed { error },
        },
        Err(error) => Ending::Failed {
            error: error.to_string(),
        },
    };
    let open = |ending| Settled {
        ending,
        closed: None,
    };
    match ending {
        Ending::Done { text, limit } => match workspace.unlandable().await {
            Some(error) => open(Ending::Retained {
                error,
                workspace: workspace.dir(),
            }),
            None if land => open(Ending::Done { text, limit }),
            // Work that cannot be read back is retained for recovery,
            // like work that cannot be checked.
            None => match workspace.keep_work(run, base).await {
                Ok(changes) => Settled {
                    ending: Ending::Kept {
                        text,
                        limit,
                        changes,
                    },
                    closed: Some(workspace.dir()),
                },
                Err(error) => open(Ending::Retained {
                    error: error.to_string(),
                    workspace: workspace.dir(),
                }),
            },
        },
        ending => match workspace.drop_work(run, base).await {
            Ok(()) => Settled {
                ending,
                closed: Some(workspace.dir()),
            },
            Err(error) => open(Ending::Failed {
                error: format!(
                    "{}; its changes could not be dropped: {error}",
                    failure(&ending)
                ),
            }),
        },
    }
}

/// What an ending that dropped the work says.
fn failure(ending: &Ending) -> String {
    match ending {
        Ending::Done { .. } | Ending::Kept { .. } => "It finished".into(),
        Ending::Failed { error } => format!("It failed: {error}"),
        Ending::Stopped => "The person stopped it".into(),
        Ending::Retained { error, workspace } => format!(
            "It could not finalize its work: {error}. Its workspace was \
             kept at {}, with its commits, for recovery",
            workspace.display()
        ),
    }
}

/// The `spawn` tool's arguments, with `models` as the ones a call may
/// pick, and `notes` the VCS's words.
fn spawn_parameters(notes: &SpawnNotes, models: &[String]) -> Value {
    let efforts: Vec<&str> = ReasoningEffort::ALL
        .iter()
        .map(|effort| effort.as_str())
        .collect();
    json!({
        "type": "object",
        "properties": {
            "task": {
                "type": "string",
                "description": "What the sub-agent does. It sees \
                    this conversation, so name the task and what \
                    sets it apart from the others you spawn; a fresh \
                    one sees only this, so write it in full.",
            },
            "fresh": {
                "type": "boolean",
                "description": "Start it without this conversation: \
                    it sees only `task`, so it judges without your \
                    leanings, as a reviewer or a judge should. False \
                    if left out.",
            },
            "land": {
                "type": "boolean",
                "description": format!(
                    "Whether its commits land on yours. False keeps them \
                     beside yours instead: {}. True if left out.",
                    notes.kept
                ),
            },
            "model": {
                "type": "string",
                "enum": models,
                "description": "The model it runs on; yours if \
                    left out.",
            },
            "effort": {
                "type": "string",
                "enum": efforts,
                "description": "Its reasoning effort; yours if \
                    left out.",
            },
        },
        "required": ["task"],
        "additionalProperties": false,
    })
}

/// What `spawn` says it does, with `notes` the VCS's words.
fn spawn_description(notes: &SpawnNotes) -> String {
    format!(
        "Start a sub-agent on a task, and go on at once: nothing waits for \
         it. It forks this conversation, so it knows what you know and \
         `task` can be short; with `fresh` it starts from `task` alone. \
         With `land` false its commits stay beside yours, for candidates \
         you compare before taking one. It works in a chat of its own on a \
         copy of your committed code: {}. Up to 8 run at once; give each \
         its own part, so their changes clash little. Spawn every task \
         that can start now, then end your turn: when one finishes, its \
         commits land on top of yours and tau starts a turn of yours with \
         what it did. Changes that clash with yours land as conflicts for \
         you to resolve. It runs on your model and effort unless `model` \
         or `effort` say otherwise; another model cannot reuse your prompt \
         cache, so it reads this whole conversation at full price.",
        notes.commit_first
    )
}

/// What a call to `spawn` below the main chat answers.
pub const ONLY_MAIN_SPAWNS: &str = "Only the main chat starts \
    sub-agents; do this work here or ask the person to start a chat.";

/// A workspace name for a sub-agent of `parent`: its caller's name and
/// twelve random hex digits, so no process takes over a workspace an
/// earlier one left behind.
fn child_name(parent: &str) -> String {
    // A v7 uuid ends in random bits; its start is the time.
    let id = uuid::Uuid::now_v7().simple().to_string();
    format!("{parent}-sub-{}", &id[20..])
}

/// Starts a sub-agent working on the caller's code. Build one per run,
/// on that run's workspace and the caller's [`SubAgents`].
pub struct Spawn {
    parent: Arc<dyn RunWorkspace>,
    agents: SubAgents,
    child: ChildAgent,
    description: String,
    parameters: Value,
}

impl Spawn {
    /// `notes` are the VCS's words for its description, and `models` the
    /// ids a call may pick a model from.
    pub fn new(
        parent: Arc<dyn RunWorkspace>,
        agents: SubAgents,
        notes: &SpawnNotes,
        models: &[String],
        child: impl Fn(Arc<dyn RunWorkspace>, &ChildModel) -> ChildFuture
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            parent,
            agents,
            child: Arc::new(child),
            description: spawn_description(notes),
            parameters: spawn_parameters(notes, models),
        }
    }
}

#[async_trait]
impl AgentTool for Spawn {
    fn name(&self) -> &str {
        SPAWN
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    // It reads the caller's working copy: the batch's other tools, a
    // `vcs_commit` among them, run before or after it, never beside.
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Grouped
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let task = args["task"].as_str().unwrap_or_default().to_owned();
        let fresh = args["fresh"].as_bool().unwrap_or(false);
        let land = args["land"].as_bool().unwrap_or(true);
        let asked = ChildModel {
            model: args["model"].as_str().map(str::to_owned),
            effort: args["effort"].as_str().and_then(ReasoningEffort::parse),
        };
        // The caller's committed work, as the sub-agent's base. The
        // model makes the commits (ADR 0014): with work not committed,
        // the sub-agent could not see it, so the VCS refuses.
        let child = self
            .parent
            .fork_child(child_name(self.parent.name()))
            .await?;
        let Some(slot) = self.agents.slot() else {
            return Err(format!(
                "{MAX_RUNNING} sub-agents are running already, the most at \
                 once: end your turn, and spawn again when tau reports one."
            )
            .into());
        };
        let agent = (self.child)(child.workspace.clone(), &asked).await?;
        let tool = agent.as_tool(SPAWN, "");
        let tool = if fresh { tool } else { tool.forking() };
        let run = tool
            .spawn(task.clone(), &ctx)
            .map_err(|error| ToolError::from(error.to_string()))?;
        let id = run.id();
        self.agents
            .follow(run, task, child.workspace, child.base, land, slot);
        let lands = if land {
            "its commits land on yours"
        } else {
            "tau reports its commits, kept beside yours"
        };
        Ok(ToolOutput {
            details: Some(json!({ "run": id.0.as_ref() })),
            ..ToolOutput::text(format!(
                "Sub-agent {id} started. It works on its own: when it \
                 finishes, {lands} and tau starts a turn of yours with what \
                 it did. Spawn the other tasks that can start now, then end \
                 your turn."
            ))
        })
    }
}

/// `spawn` on a run below the main chat. Runs nest one level (ADR
/// 0016), so every call fails with [`ONLY_MAIN_SPAWNS`]. It is declared
/// all the same, with the main chat's description and arguments, so the
/// run's tools match main's and it can read main's prompt cache (ADR
/// 0022).
pub struct RefusingSpawn {
    description: String,
    parameters: Value,
}

impl RefusingSpawn {
    /// `notes` and `models` are those the main chat's `spawn` has.
    pub fn new(notes: &SpawnNotes, models: &[String]) -> Self {
        Self {
            description: spawn_description(notes),
            parameters: spawn_parameters(notes, models),
        }
    }
}

#[async_trait]
impl AgentTool for RefusingSpawn {
    fn name(&self) -> &str {
        SPAWN
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Grouped
    }

    async fn call(
        &self,
        _args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        Err(ONLY_MAIN_SPAWNS.into())
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    /// Where the model says the sub-agent is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Reading,
        Ending,
        Checking,
        Ended,
        Taken,
    }

    /// A sub-agent's life against a model of it: its run ends (with or
    /// without its `RunEnd` seen first), its work is checked and settles,
    /// while the person stops it and the host takes it at any point.
    struct Model {
        life: StateMachine<Life>,
        phase: Phase,
        /// A stop was heard.
        heard: bool,
        /// How it ended, once it did.
        ending: Option<Ending>,
    }

    impl Model {
        fn tell(&mut self, event: &Event) -> Answer {
            let mut answer = Answer::default();
            self.life.handle_with_context(event, &mut answer);
            answer
        }
    }

    #[hegel::state_machine]
    impl Model {
        /// The person stops it: heard until its work is checked, and the
        /// run is cancelled only while it still reads.
        #[rule(weight = 3)]
        fn stop(&mut self, _tc: TestCase) {
            let answer = self.tell(&Event::Stop);
            let in_time = matches!(self.phase, Phase::Reading | Phase::Ending);
            assert_eq!(answer.stopped, in_time, "a stop at {:?}", self.phase);
            assert_eq!(answer.cancel, self.phase == Phase::Reading);
            self.heard |= in_time;
        }

        #[rule]
        fn run_end(&mut self, tc: TestCase) {
            if self.phase != Phase::Reading {
                tc.reject();
            }
            self.tell(&Event::RunEnd);
            self.phase = Phase::Ending;
        }

        /// Its run comes out, its `RunEnd` seen or not.
        #[rule]
        fn outcome(&mut self, tc: TestCase) {
            if !matches!(self.phase, Phase::Reading | Phase::Ending) {
                tc.reject();
            }
            self.tell(&Event::Outcome);
            self.phase = Phase::Checking;
            // The check sees every stop that was heard.
            assert!(
                matches!(
                    self.life.state(),
                    State::Checking { stopped } if *stopped == self.heard
                ),
                "{:?}",
                self.life.state()
            );
        }

        #[rule]
        fn settled(&mut self, tc: TestCase) {
            if self.phase != Phase::Checking {
                tc.reject();
            }
            let ending = if self.heard {
                Ending::Stopped
            } else {
                Ending::Done {
                    text: tc.draw(gs::text().max_size(4)),
                    limit: None,
                }
            };
            self.tell(&Event::Settled(ending.clone(), None));
            self.ending = Some(ending);
            self.phase = Phase::Ended;
        }

        /// The host takes it: one taker gets its ending, once it ended.
        #[rule(weight = 2)]
        fn take(&mut self, _tc: TestCase) {
            let taken = self.tell(&Event::Take).taken;
            let want = match (self.phase, &self.ending) {
                (Phase::Ended, Some(ending)) => Taken::Now(ending.clone()),
                (Phase::Taken, Some(ending)) => Taken::Before(ending.clone()),
                _ => Taken::Running,
            };
            assert_eq!(taken, Some(want));
            if self.phase == Phase::Ended {
                self.phase = Phase::Taken;
            }
        }

        #[invariant(always_run)]
        fn the_life_is_the_model(&self, _tc: TestCase) {
            let state = self.life.state();
            let phase = match state {
                State::Reading { .. } => Phase::Reading,
                State::Ending { .. } => Phase::Ending,
                State::Checking { .. } => Phase::Checking,
                State::Ended { .. } => Phase::Ended,
                State::Taken { .. } => Phase::Taken,
            };
            assert_eq!(phase, self.phase);
            assert_eq!(
                state.is_running(),
                matches!(
                    self.phase,
                    Phase::Reading | Phase::Ending | Phase::Checking
                )
            );
            assert_eq!(state.outcome(), self.ending.as_ref());
        }
    }

    #[hegel::test(test_cases = 500)]
    fn a_sub_agents_life_keeps_to_its_model(tc: TestCase) {
        let model = Model {
            life: Life.state_machine(),
            phase: Phase::Reading,
            heard: false,
            ending: None,
        };
        hegel::stateful::machine(model).steps(20).run(tc);
    }

    /// Stop after the `RunEnd`, before the work is checked: heard, and
    /// the check drops the work; the run is not cancelled again.
    #[test]
    fn a_stop_after_the_run_end_is_heard_by_the_check() {
        let mut life = Life.state_machine();
        life.handle_with_context(&Event::RunEnd, &mut Answer::default());
        let mut answer = Answer::default();
        life.handle_with_context(&Event::Stop, &mut answer);
        assert!(answer.stopped && !answer.cancel, "{answer:?}");
        life.handle_with_context(&Event::Outcome, &mut Answer::default());
        assert!(matches!(life.state(), State::Checking { stopped: true }));
    }

    /// Stop while the work is checked is too late, and says so.
    #[test]
    fn a_stop_while_the_work_is_checked_stops_nothing() {
        let mut life = Life.state_machine();
        life.handle_with_context(&Event::Outcome, &mut Answer::default());
        let mut answer = Answer::default();
        life.handle_with_context(&Event::Stop, &mut answer);
        assert!(!answer.stopped && !answer.cancel, "{answer:?}");
        assert!(matches!(life.state(), State::Checking { stopped: false }));
    }
}
