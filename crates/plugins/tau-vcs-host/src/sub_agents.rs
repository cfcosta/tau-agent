//! Sub-agents that run beside their caller (ADR 0009, 0015, 0026,
//! 0031): [`Spawn`] starts one and answers at once, and [`SubAgents`]
//! keeps them between the caller's turns. Nothing waits for one: the
//! host lands each as it ends.
//!
//! 1. The caller's work must be committed (ADR 0014): `spawn` refuses
//!    while `@` holds changes. The sub-agent's workspace starts on the
//!    caller's newest commit.
//! 2. The sub-agent is a run of its own under the caller, in a chat of
//!    its own. It forks the caller: it sees the conversation, the batch
//!    its call is in, and then its task. It commits its work on its own
//!    stack. Up to [`MAX_RUNNING`] run at once; the caller goes on
//!    meanwhile, and stopping the caller does not stop them.
//! 3. When one ends, its work is checked. One that finished, or that a
//!    limit stopped, can land; one that failed or was stopped has its
//!    changes dropped and closes: its workspace and bookmark go.
//! 4. The host takes each one that ended ([`SubAgents::take`]) and
//!    lands it on the caller when the caller is idle, one landing at a
//!    time. A landing that conflicts lands its conflicts for the caller
//!    to resolve.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, AgentError, Outcome, Run, RunControl},
    error::ToolError,
    event::{LimitKind, RunEvent, StopReason},
    tool::{AgentTool, ExecutionMode, RunId, ToolCtx, ToolOutput},
};
use tau_ai::responses::request::ReasoningEffort;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

use crate::{
    Landing,
    details::SPAWN,
    error::VcsError,
    run_workspace::{RunWorkspace, bookmark},
    vcs::Identity,
};

/// What a call asks of its sub-agent's model. `None` keeps the
/// caller's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildModel {
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
}

/// Builds a sub-agent's agent around its workspace, on the model its
/// call asks for: the tools and plugins it runs with, the workspace
/// among them.
pub type ChildAgent =
    Arc<dyn Fn(RunWorkspace, &ChildModel) -> ChildFuture + Send + Sync>;

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
    /// It failed: its changes were dropped.
    Failed { error: String },
    /// The person stopped it: its changes were dropped.
    Stopped,
    /// Its work could not be checked: nothing lands, and its workspace
    /// and bookmark were kept at `workspace` for recovery.
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

struct Child {
    run: RunId,
    task: String,
    control: RunControl,
    state: State,
    /// The person stopped it.
    stopped: bool,
}

#[derive(Debug, Clone)]
enum State {
    Running,
    Ended(Ending),
    Taken(Ending),
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

    /// Whether `run` is one of these and still running.
    pub fn is_running(&self, run: &RunId) -> bool {
        self.with(|children| {
            children
                .iter()
                .any(|c| c.run == *run && matches!(c.state, State::Running))
        })
    }

    /// The ones still running.
    pub fn running(&self) -> Vec<RunId> {
        self.with(|children| {
            children
                .iter()
                .filter(|c| matches!(c.state, State::Running))
                .map(|c| c.run.clone())
                .collect()
        })
    }

    /// How to steer `run`, while it runs.
    pub fn control(&self, run: &RunId) -> Option<RunControl> {
        self.with(|children| {
            children
                .iter()
                .find(|c| c.run == *run && matches!(c.state, State::Running))
                .map(|c| c.control.clone())
        })
    }

    /// Stops `run`, if it is running: its changes are dropped. Returns
    /// whether it was.
    pub fn stop(&self, run: &RunId) -> bool {
        self.with(|children| {
            let Some(child) = children
                .iter_mut()
                .find(|c| c.run == *run && matches!(c.state, State::Running))
            else {
                return false;
            };
            child.stopped = true;
            child.control.cancel();
            true
        })
    }

    /// Waits for `run` to end and its work to be checked. `None` when it
    /// is not one of these.
    pub async fn ended(&self, run: &RunId) -> Option<Ending> {
        let mut changed = self.0.changed.subscribe();
        loop {
            let state = self.with(|children| {
                children
                    .iter()
                    .find(|c| c.run == *run)
                    .map(|c| c.state.clone())
            })?;
            match state {
                State::Running => {}
                State::Ended(ending) | State::Taken(ending) => {
                    return Some(ending);
                }
            }
            if changed.changed().await.is_err() {
                return None;
            }
        }
    }

    /// Takes `run`, once it ended, to land it or report it: one taker
    /// gets it, the others learn it went.
    pub fn take(&self, run: &RunId) -> Taken {
        self.with(|children| {
            let Some(child) = children.iter_mut().find(|c| c.run == *run)
            else {
                return Taken::Unknown;
            };
            match &child.state {
                State::Running => Taken::Running,
                State::Taken(ending) => Taken::Before(ending.clone()),
                State::Ended(ending) => {
                    let ending = ending.clone();
                    child.state = State::Taken(ending.clone());
                    Taken::Now(ending)
                }
            }
        })
    }

    /// Whether someone took `run` ([`Self::take`]); `None` when it is
    /// not one of these.
    pub fn taken(&self, run: &RunId) -> Option<bool> {
        self.with(|children| {
            children
                .iter()
                .find(|c| c.run == *run)
                .map(|c| matches!(c.state, State::Taken(_)))
        })
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
    /// `task`: passes its events on, then checks its work as it ends.
    fn follow(
        &self,
        mut run: Run,
        task: String,
        workspace: RunWorkspace,
        base: String,
        slot: OwnedSemaphorePermit,
    ) {
        let id = run.id();
        self.with(|children| {
            children.push(Child {
                run: id.clone(),
                task,
                control: run.control(),
                state: State::Running,
                stopped: false,
            })
        });
        let agents = self.clone();
        tokio::spawn(async move {
            {
                let mut events = run.events();
                while let Some(event) = events.next().await {
                    if let Some(sink) = &agents.0.events {
                        let _ = sink.send(event);
                    }
                }
            }
            let outcome = run.outcome().await;
            drop(slot);
            let stopped = agents.with(|children| {
                children.iter().any(|c| c.run == id && c.stopped)
            });
            let ending = settle(outcome, stopped, &id, &workspace, &base).await;
            agents.with(|children| {
                if let Some(child) = children.iter_mut().find(|c| c.run == id) {
                    child.state = State::Ended(ending.clone());
                }
            });
            agents.0.changed.send_modify(|n| *n += 1);
            if let Some(on_end) = &agents.0.on_end {
                on_end(&id, &ending);
            }
        });
    }
}

/// How a sub-agent's run came out, once its work is checked: work that
/// can land is committed; a failed or stopped one's changes are dropped
/// and it closes.
async fn settle(
    outcome: Result<Outcome, AgentError>,
    stopped: bool,
    run: &RunId,
    workspace: &RunWorkspace,
    base: &str,
) -> Ending {
    let ending = match outcome {
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
            StopReason::Cancelled if stopped => Ending::Stopped,
            StopReason::Cancelled => Ending::Failed {
                error: "it was cancelled".into(),
            },
            StopReason::Error(error) => Ending::Failed { error },
        },
        Err(_) if stopped => Ending::Stopped,
        Err(error) => Ending::Failed {
            error: error.to_string(),
        },
    };
    match ending {
        Ending::Done { .. } => match unlandable(workspace).await {
            Some(error) => Ending::Retained {
                error,
                workspace: workspace.dir(),
            },
            None => ending,
        },
        ending => {
            if let Err(error) = drop_work(workspace, run, base).await {
                return Ending::Failed {
                    error: format!(
                        "{}; its changes could not be dropped: {error}",
                        failure(&ending)
                    ),
                };
            }
            ending
        }
    }
}

/// Why `workspace`'s work cannot land, if it cannot: a normal or limited
/// outcome does not prove the finish hook committed everything, so it is
/// checked through jj-lib before any landing, abandonment or workspace
/// deletion.
async fn unlandable(workspace: &RunWorkspace) -> Option<String> {
    if let Some(error) = workspace.finalization_error() {
        return Some(error);
    }
    match workspace.vcs().working_copy().await {
        Ok(copy) if copy.is_committed() => None,
        Ok(copy) => Some(format!(
            "uncommitted paths: {}; oversized untracked paths: {}",
            copy.paths.join(", "),
            copy.too_large
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )),
        Err(error) => Some(error.to_string()),
    }
}

/// Abandons a sub-agent's own changes, from `base` up, and closes it:
/// its workspace and bookmark go.
async fn drop_work(
    workspace: &RunWorkspace,
    run: &RunId,
    base: &str,
) -> Result<(), VcsError> {
    let (base, name, bookmark) =
        (base.to_owned(), workspace.name().to_owned(), bookmark(run));
    workspace
        .project()
        .run(move |p| {
            if let Some(wc) = p.workspace_head(&name)? {
                p.abandon_between(&base, &wc)?;
            }
            p.forget_workspace(&name)?;
            p.remove_bookmark(&bookmark)?;
            Ok(())
        })
        .await
}

/// What an ending that dropped the work says.
fn failure(ending: &Ending) -> String {
    match ending {
        Ending::Done { .. } => "It finished".into(),
        Ending::Failed { error } => format!("It failed: {error}"),
        Ending::Stopped => "The person stopped it".into(),
        Ending::Retained { error, workspace } => format!(
            "It could not finalize its work: {error}. Its workspace and \
             bookmark were kept at {} for recovery",
            workspace.display()
        ),
    }
}

/// The `spawn` tool's arguments, with `models` as the ones a call may
/// pick.
fn spawn_parameters(models: &[String]) -> Value {
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
                    sets it apart from the others you spawn.",
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

/// What `spawn` says it does.
const SPAWN_DESCRIPTION: &str = "Start a sub-agent on a task, and go on \
    at once: nothing waits for it. It forks this conversation, so it \
    knows what you know and `task` can be short. It works in a chat of \
    its own on a copy of your committed code: commit your work with \
    `vcs_commit` first. Up to 8 run at once; give each its own part, so \
    their changes clash little. Spawn every task that can start now, \
    then end your turn: when one finishes, its commits land on top of \
    yours and tau starts a turn of yours with what it did. Changes that \
    clash with yours land as conflicts for you to resolve. It runs on \
    your model and effort unless `model` or `effort` say otherwise; \
    another model cannot reuse your prompt cache, so it reads this \
    whole conversation at full price.";

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
/// on that run's [`RunWorkspace`] and the caller's [`SubAgents`].
pub struct Spawn {
    parent: RunWorkspace,
    identity: Identity,
    agents: SubAgents,
    child: ChildAgent,
    parameters: Value,
}

impl Spawn {
    /// `models` are the ids a call may pick a model from.
    pub fn new(
        parent: RunWorkspace,
        identity: Identity,
        agents: SubAgents,
        models: &[String],
        child: impl Fn(RunWorkspace, &ChildModel) -> ChildFuture
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            parent,
            identity,
            agents,
            child: Arc::new(child),
            parameters: spawn_parameters(models),
        }
    }
}

#[async_trait]
impl AgentTool for Spawn {
    fn name(&self) -> &str {
        SPAWN
    }

    fn description(&self) -> &str {
        SPAWN_DESCRIPTION
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
        let asked = ChildModel {
            model: args["model"].as_str().map(str::to_owned),
            effort: args["effort"].as_str().and_then(ReasoningEffort::parse),
        };
        // The caller's committed work, as the sub-agent's base. The
        // model makes the commits (ADR 0014): with work still in `@`, the
        // sub-agent could not see it, so it commits first.
        let working_copy = self.parent.vcs().working_copy().await?;
        if !working_copy.too_large.is_empty() {
            return Err(VcsError::UntrackedLarge(
                working_copy
                    .too_large
                    .iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
            .into());
        }
        if !working_copy.paths.is_empty() {
            return Err(
                VcsError::Uncommitted(working_copy.paths.join(", ")).into()
            );
        }
        let head = working_copy.head;
        let Some(slot) = self.agents.slot() else {
            return Err(format!(
                "{MAX_RUNNING} sub-agents are running already, the most at \
                 once: end your turn, and spawn again when tau reports one."
            )
            .into());
        };
        let workspace = RunWorkspace::new(
            self.parent.project().clone(),
            child_name(self.parent.name()),
            self.identity.clone(),
        )?
        .with_base(head.clone());
        let agent = (self.child)(workspace.clone(), &asked).await?;
        let run = agent
            .as_tool(SPAWN, "")
            .forking()
            .spawn(task.clone(), &ctx)
            .map_err(|error| ToolError::from(error.to_string()))?;
        let id = run.id();
        self.agents.follow(run, task, workspace, head, slot);
        Ok(ToolOutput {
            details: Some(json!({ "run": id.0.as_ref() })),
            ..ToolOutput::text(format!(
                "Sub-agent {id} started. It works on its own: when it \
                 finishes, its commits land on yours and tau starts a turn \
                 of yours with what it did. Spawn the other tasks that can \
                 start now, then end your turn."
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
    parameters: Value,
}

impl RefusingSpawn {
    /// `models` are the ids the main chat's `spawn` offers.
    pub fn new(models: &[String]) -> Self {
        Self {
            parameters: spawn_parameters(models),
        }
    }
}

#[async_trait]
impl AgentTool for RefusingSpawn {
    fn name(&self) -> &str {
        SPAWN
    }

    fn description(&self) -> &str {
        SPAWN_DESCRIPTION
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

/// A limit, as the result's details name it.
pub fn limit_name(limit: LimitKind) -> &'static str {
    match limit {
        LimitKind::Turns => "turns",
        LimitKind::Tokens => "tokens",
        LimitKind::Usd => "cost",
        LimitKind::Time => "time",
    }
}

/// What the caller reads about a landing, after the sub-agent's answer:
/// whether a limit cut the sub-agent short, how many changes landed, and
/// the paths this landing left in conflict (`brought`), not those the
/// caller's head held already.
pub fn landing_note(
    landing: &Landing,
    brought: &[String],
    limit: Option<LimitKind>,
) -> String {
    let short = match limit {
        Some(LimitKind::Turns) => "It stopped at its turn limit. ",
        Some(LimitKind::Tokens) => "It stopped at its token limit. ",
        Some(LimitKind::Usd) => "It stopped at its cost limit. ",
        Some(LimitKind::Time) => "It stopped at its time limit. ",
        None => "",
    };
    format!("[{short}{}]", landed_note(landing, brought))
}

fn landed_note(landing: &Landing, brought: &[String]) -> String {
    let landed = match landing.changes.len() {
        0 => return "It changed no files.".to_owned(),
        1 => "Its 1 change landed on top of yours".to_owned(),
        n => format!("Its {n} changes landed on top of yours"),
    };
    if brought.is_empty() {
        return format!("{landed}.");
    }
    format!(
        "{landed}, with conflicts in {}: resolve their conflict markers, \
         then commit.",
        brought.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use hegel::generators as gs;

    use super::*;
    use crate::ChangeInfo;

    /// The note says which limit cut the sub-agent short, if one did, how
    /// many changes landed and, when the landing brought conflicts, names
    /// those paths and no others; a landing with nothing in it says so,
    /// whatever else it holds.
    #[hegel::test(test_cases = 200)]
    fn the_note_says_what_landed(tc: hegel::TestCase) {
        let changes: usize = tc.draw(gs::integers().max_value(5));
        let conflicts: Vec<String> = tc.draw(
            gs::vecs(gs::from_regex("[a-z]{1,6}\\.rs").fullmatch(true))
                .max_size(4)
                .unique(true),
        );
        let brought: Vec<String> = tc.draw(gs::subsequences(conflicts.clone()));
        let limits = [
            None,
            Some((LimitKind::Turns, "turn")),
            Some((LimitKind::Tokens, "token")),
            Some((LimitKind::Usd, "cost")),
            Some((LimitKind::Time, "time")),
        ];
        let limit: usize = tc.draw(gs::integers().max_value(limits.len() - 1));
        let limit = limits[limit];
        let change = |n: usize| ChangeInfo {
            change_id: format!("k{n}"),
            commit_id: format!("c{n}"),
            description: String::new(),
            empty: false,
            conflict: false,
            immutable: false,
            working_copy: false,
            divergent: false,
            bookmarks: Vec::new(),
        };
        let landing = Landing {
            changes: (0..changes).map(change).collect(),
            conflicts: conflicts.clone(),
            head: "h".into(),
        };
        let note = landing_note(&landing, &brought, limit.map(|(l, _)| l));
        // A limit that cut it short comes first.
        let note = match limit {
            Some((_, name)) => {
                let short = format!("[It stopped at its {name} limit. ");
                assert!(note.starts_with(&short), "{note}");
                format!("[{}", &note[short.len()..])
            }
            None => note,
        };
        if changes == 0 {
            assert_eq!(note, "[It changed no files.]");
            return;
        }
        let count = if changes == 1 {
            "Its 1 change landed".to_owned()
        } else {
            format!("Its {changes} changes landed")
        };
        assert!(note.starts_with(&format!("[{count}")), "{note}");
        assert_eq!(note.contains("conflicts"), !brought.is_empty(), "{note}");
        let named: Vec<&str> = note
            .split_once("with conflicts in ")
            .and_then(|(_, rest)| rest.split_once(':'))
            .map(|(list, _)| list.split(", ").collect())
            .unwrap_or_default();
        assert_eq!(named, brought, "{note}");
    }
}
