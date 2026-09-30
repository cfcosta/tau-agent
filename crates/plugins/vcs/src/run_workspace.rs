//! [`RunWorkspace`]: a run's own jj workspace in a [`Project`]
//! (`docs/reference/vcs.md`, "Runs and turns").
//!
//! When the run starts, the plugin makes its workspace: on the project's
//! trunk for a new run, or from the snapshot of the turn a fork
//! continues from. The model makes the commits, with `vcs_commit`
//! ([ADR 0014](../../../../docs/decisions/0014-the-model-commits-and-runs-land-as-stacked-diffs.md)):
//! after each turn the plugin only snapshots `@` and stores a [`Link`]
//! from the turn to that snapshot, so a fork of the run at a turn starts
//! on that turn's code. A run that would stop with uncommitted work is
//! asked once to commit it; what is left at the end is committed with a
//! message the run's model writes from the diff. Whoever else cares what
//! a turn changed, such as memory marking notes about those files stale,
//! hears of each turn through [`RunWorkspace::on_turn`].

use std::{
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tau_agent::{
    error::PluginError,
    event::{RunEvent, StopReason},
    plugin::{
        FinishedRun,
        Plugin,
        PluginCtx,
        PluginRun,
        RunPlan,
        StopDecision,
    },
    tool::RunId,
};
use tau_ai::{
    message::{
        AssistantBlock,
        AssistantMessage,
        Message,
        UserContent,
        UserMessage,
    },
    responses::request::Settings,
};

use crate::{
    TurnSnapshot,
    error::VcsError,
    project::Project,
    vcs::{Identity, Vcs},
};

/// Hears of each turn's snapshot.
pub type TurnObserver = Arc<dyn Fn(&TurnSnapshot) + Send + Sync>;

/// What the model is told when it would stop with uncommitted work.
pub const COMMIT_FIRST: &str = "You have uncommitted changes. Commit your work with `vcs_commit`, \
     with a message a reviewer can read, before you finish.";

/// What the run's model is asked, to describe work it left uncommitted.
const DESCRIBE: &str = "Write the commit message for this change, in Conventional Commits \
style: a subject line `type(scope): summary` of at most 72 characters, a \
blank line, then a short body that says what changed and why. Answer \
with the message only.";

/// The name the plugin stores its links under.
pub const PLUGIN: &str = "workspace";

/// The local bookmark on a run's newest commit: `tau/<run id>`.
pub fn bookmark(run: &RunId) -> String {
    format!("tau/{}", run.0)
}

/// A turn, and the commit that holds the files it left. Stored as the
/// plugin's record after each turn, and for each change a landing
/// brought.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub turn: u32,
    pub workspace: String,
    pub commit_id: String,
    pub change_id: String,
    /// The turn changed files.
    pub changed: bool,
    /// The child run whose landing put this change on the run's stack
    /// (ADR 0009); none for the run's own turns. A landing stores one
    /// link per change it brought, all with the run's latest turn.
    pub from: Option<String>,
    /// The commit is a snapshot of `@` at the end of the turn, found by
    /// its commit id: `@` moves on under the same change id (ADR 0014).
    /// Otherwise it is a change, found by its change id.
    #[serde(default)]
    pub snapshot: bool,
}

impl Link {
    /// Reads a link back from a stored record body.
    pub fn parse(body: &str) -> Option<Self> {
        serde_json::from_str(body).ok()
    }
}

/// A change that came to the run's stack in the middle of a turn: the
/// run's own work up to a delegated task, or a change the task landed.
/// It is linked when the turn ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pending {
    pub commit_id: String,
    pub change_id: String,
    pub from: Option<String>,
}

/// A run's workspace, as a plugin. Build one per run, with a workspace
/// name of its own, and point the run's other tools at [`Self::dir`].
/// Clones share what they learn of the run.
#[derive(Clone)]
pub struct RunWorkspace {
    project: Project,
    name: String,
    vcs: Vcs,
    observers: Vec<TurnObserver>,
    /// Where a new run's workspace starts instead of trunk.
    base: Option<String>,
    pending: Arc<Mutex<Vec<Pending>>>,
    /// The run the workspace serves, once it has started.
    run: Arc<Mutex<Option<RunId>>>,
}

impl fmt::Debug for RunWorkspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunWorkspace")
            .field("project", &self.project)
            .field("name", &self.name)
            .field("vcs", &self.vcs)
            .field("observers", &self.observers.len())
            .finish()
    }
}

impl RunWorkspace {
    pub fn new(
        project: Project,
        name: impl Into<String>,
        identity: Identity,
    ) -> Result<Self, VcsError> {
        let name = name.into();
        let vcs = Vcs::lazy(project.workspace_dir(&name), identity)?;
        Ok(Self {
            project,
            name,
            vcs,
            observers: Vec::new(),
            base: None,
            pending: Arc::default(),
            run: Arc::default(),
        })
    }

    /// Starts a new run's workspace on `commit` (a full commit id in
    /// hex) rather than on trunk: a sub-agent's, on its caller's work.
    /// A fork still starts on the turn it continues from.
    pub fn with_base(mut self, commit: impl Into<String>) -> Self {
        self.base = Some(commit.into());
        self
    }

    /// The run this workspace serves, once it has started.
    pub fn run(&self) -> Option<RunId> {
        self.run.lock().expect("not poisoned").clone()
    }

    pub(crate) fn project(&self) -> &Project {
        &self.project
    }

    /// Changes to link when the current turn ends, before its own.
    pub(crate) fn queue(&self, changes: impl IntoIterator<Item = Pending>) {
        self.pending.lock().expect("not poisoned").extend(changes);
    }

    /// Calls `observer` with each turn's snapshot, after it is made.
    pub fn on_turn(
        mut self,
        observer: impl Fn(&TurnSnapshot) + Send + Sync + 'static,
    ) -> Self {
        self.observers.push(Arc::new(observer));
        self
    }

    /// Where the run's files are, once it has started.
    pub fn dir(&self) -> PathBuf {
        self.project.workspace_dir(&self.name)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The workspace, for [`crate::VcsPlugin`]. It loads on first use,
    /// so it can be handed out before the run starts.
    pub fn vcs(&self) -> &Vcs {
        &self.vcs
    }
}

#[async_trait]
impl Plugin for RunWorkspace {
    fn name(&self) -> &str {
        PLUGIN
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        *self.run.lock().expect("not poisoned") = Some(ctx.run.clone());
        // A fork continues from the last turn it inherits.
        let inherited = plan.records().iter().rev().find_map(|record| {
            serde_json::from_value::<Link>(record.clone()).ok()
        });
        let since = inherited
            .as_ref()
            .filter(|link| link.snapshot)
            .map(|link| link.commit_id.clone());
        let project = self.project.clone();
        let name = self.name.clone();
        let base = self.base.clone();
        tokio::task::spawn_blocking(move || match (inherited, base) {
            (Some(link), _) if link.snapshot => project
                .add_workspace_from_snapshot(&name, &link.commit_id)
                .map(|_| ()),
            // The change may have been restacked since.
            (Some(link), _) => {
                let base = project.current([link])?.remove(0).commit_id;
                project.add_workspace(&name, &base).map(|_| ())
            }
            (None, Some(base)) => {
                project.add_workspace(&name, &base).map(|_| ())
            }
            (None, None) => {
                let trunk = project.trunk()?;
                project.add_workspace(&name, &trunk).map(|_| ())
            }
        })
        .await??;
        Ok(Box::new(Turns {
            vcs: self.vcs.clone(),
            name: self.name.clone(),
            observers: self.observers.clone(),
            pending: self.pending.clone(),
            model: plan.model().to_owned(),
            since,
            held: false,
        }))
    }
}

struct Turns {
    vcs: Vcs,
    name: String,
    observers: Vec<TurnObserver>,
    pending: Arc<Mutex<Vec<Pending>>>,
    /// The run's model, which describes work left uncommitted.
    model: String,
    /// The last turn's snapshot, to tell what a turn changed.
    since: Option<String>,
    /// The run was asked once to commit before stopping.
    held: bool,
}

#[async_trait]
impl PluginRun for Turns {
    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        let RunEvent::TurnEnd { run, turn, .. } = event else {
            return;
        };
        if run != &ctx.run {
            return;
        }
        // What came to the stack during the turn, in order, before the
        // turn's snapshot.
        let pending: Vec<Pending> =
            std::mem::take(&mut *self.pending.lock().expect("not poisoned"));
        for change in pending {
            let link = Link {
                turn: *turn,
                workspace: self.name.clone(),
                commit_id: change.commit_id,
                change_id: change.change_id,
                changed: true,
                from: change.from,
                snapshot: false,
            };
            let _ = ctx
                .record(&serde_json::to_value(link).unwrap_or_default())
                .await;
        }
        let record = match self
            .vcs
            .end_turn(bookmark(&ctx.run), self.since.clone())
            .await
        {
            Ok(snapshot) => {
                for observer in &self.observers {
                    observer(&snapshot);
                }
                self.since = Some(snapshot.commit_id.clone());
                serde_json::to_value(Link {
                    turn: *turn,
                    workspace: self.name.clone(),
                    commit_id: snapshot.commit_id,
                    change_id: snapshot.change_id,
                    changed: !snapshot.paths.is_empty(),
                    from: None,
                    snapshot: true,
                })
                .unwrap_or_default()
            }
            // Not fatal to the run: a turn without a link cannot be
            // forked from.
            Err(error) => serde_json::json!({
                "turn": turn,
                "error": error.to_string(),
            }),
        };
        let _ = ctx.record(&record).await;
    }

    /// Asks once for uncommitted work to be committed.
    async fn before_stop(
        &mut self,
        _message: &AssistantMessage,
        _ctx: &PluginCtx,
    ) -> Result<StopDecision, PluginError> {
        if self.held {
            return Ok(StopDecision::Stop);
        }
        let working_copy = self.vcs.working_copy().await?;
        if working_copy.paths.is_empty() {
            return Ok(StopDecision::Stop);
        }
        self.held = true;
        Ok(StopDecision::Continue(format!(
            "{COMMIT_FIRST} Uncommitted: {}.",
            working_copy.paths.join(", ")
        )))
    }

    /// Commits what the run left uncommitted, with a message its model
    /// writes, so no work stays outside a change.
    async fn finish(&mut self, run: &FinishedRun<'_>, ctx: &PluginCtx) {
        // A run that failed or was cancelled is not done: its work stays
        // as it is, for the person to look at.
        if !matches!(run.stop, StopReason::Stop | StopReason::Limit(_)) {
            return;
        }
        let Ok(working_copy) = self.vcs.working_copy().await else {
            return;
        };
        if working_copy.paths.is_empty() {
            return;
        }
        let Ok(diff) = self.vcs.working_copy_diff().await else {
            return;
        };
        let task = run
            .transcript
            .iter()
            .find_map(|message| match message {
                Message::User(user) => match &user.content {
                    UserContent::Text(text) => Some(text.clone()),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or_default();
        let settings = Settings {
            model: self.model.clone(),
            instructions: Some(DESCRIBE.to_owned()),
            ..Settings::default()
        };
        let input = [Message::User(UserMessage {
            content: UserContent::Text(format!(
                "<task>\n{task}\n</task>\n<diff>\n{diff}\n</diff>"
            )),
            timestamp: ctx.now(),
        })];
        // Without a message from a model, the work stays uncommitted:
        // tau never writes one itself.
        let Ok(answer) = ctx.ask(settings, &input).await else {
            return;
        };
        let message: String = answer
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        if message.trim().is_empty() {
            return;
        }
        let _ = self
            .vcs
            .commit_all(message.trim(), bookmark(&ctx.run))
            .await;
    }
}
