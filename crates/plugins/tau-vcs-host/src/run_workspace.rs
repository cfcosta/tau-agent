//! [`RunWorkspace`]: a run's own jj workspace in a [`ProjectRepo`]
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
    message::{AssistantMessage, Message, UserContent, UserMessage},
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
pub const PLUGIN: &str = crate::ui::NAME;

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
    /// The run the workspace serves, once it has started.
    run: Arc<Mutex<Option<RunId>>>,
    /// The bookmark the run's commits move, instead of `tau/<run>`: a
    /// main chat's, which commits on trunk.
    commits_to: Option<String>,
    /// A failed end-of-run commit, shared with the caller that spawned it.
    finalization_error: Arc<Mutex<Option<String>>>,
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
            run: Arc::default(),
            commits_to: None,
            finalization_error: Arc::default(),
        })
    }

    /// Moves `bookmark` with the run's commits instead of `tau/<run>`:
    /// a repository's main chat commits on trunk's bookmark.
    pub fn commits_to(mut self, bookmark: impl Into<String>) -> Self {
        self.commits_to = Some(bookmark.into());
        self
    }

    /// The bookmark `run`'s commits move: [`Self::commits_to`]'s, else
    /// [`bookmark`]'s.
    pub fn bookmark_of(&self, run: &RunId) -> String {
        self.commits_to.clone().unwrap_or_else(|| bookmark(run))
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

    pub(crate) fn finalization_error(&self) -> Option<String> {
        self.finalization_error
            .lock()
            .expect("not poisoned")
            .clone()
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
        self.finalization_error.lock().expect("not poisoned").take();
        // A fork continues from the last turn it inherits.
        let inherited = plan.records().iter().rev().find_map(|record| {
            serde_json::from_value::<Link>(record.clone()).ok()
        });
        let inherited_snapshot = inherited
            .as_ref()
            .filter(|link| link.snapshot)
            .map(|link| link.commit_id.clone());
        let project = self.project.clone();
        let name = self.name.clone();
        let base = self.base.clone();
        // `@` as the run starts: what its first turn changed is told
        // from it. A fork's `@` holds its turn's files, merged onto the
        // parent as it is now, which the fork's own turn did not change.
        let start = project
            .run(move |project| {
                match (inherited, base) {
                    (Some(link), _) if link.snapshot => project
                        .add_workspace_from_snapshot(&name, &link.commit_id)
                        .map(|_| ())?,
                    // The change may have been restacked since.
                    (Some(link), _) => {
                        let base = project.current([link])?.remove(0).commit_id;
                        project.add_workspace(&name, &base).map(|_| ())?
                    }
                    (None, Some(base)) => {
                        project.add_workspace(&name, &base).map(|_| ())?
                    }
                    (None, None) => {
                        let trunk = project.trunk()?;
                        project.add_workspace(&name, &trunk).map(|_| ())?
                    }
                }
                project.workspace_head(&name)
            })
            .await?;
        let since = start.or(inherited_snapshot);
        Ok(Box::new(Turns {
            vcs: self.vcs.clone(),
            name: self.name.clone(),
            observers: self.observers.clone(),
            model: plan.model().to_owned(),
            task: plan.input.clone(),
            since,
            held: false,
            bookmark: self.bookmark_of(&ctx.run),
            finalization_error: self.finalization_error.clone(),
        }))
    }
}

struct Turns {
    vcs: Vcs,
    name: String,
    observers: Vec<TurnObserver>,
    /// The run's model, which describes work left uncommitted.
    model: String,
    /// What the run was asked: its own input, not the first message of a
    /// transcript it inherited.
    task: String,
    /// The last turn's snapshot, or `@` as the run started, to tell what
    /// a turn changed.
    since: Option<String>,
    /// The run was asked once to commit before stopping.
    held: bool,
    /// The bookmark the run's commits move.
    bookmark: String,
    finalization_error: Arc<Mutex<Option<String>>>,
}

impl Turns {
    async fn commit_pending(&self, ctx: &PluginCtx) -> Result<(), PluginError> {
        let working_copy = self.vcs.working_copy().await?;
        if working_copy.is_committed() {
            return Ok(());
        }
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
        let diff = self.vcs.working_copy_diff().await?;
        let settings = Settings {
            model: self.model.clone(),
            instructions: Some(DESCRIBE.to_owned()),
            ..Settings::default()
        };
        let input = [Message::User(UserMessage {
            content: UserContent::Text(format!(
                "<task>\n{}\n</task>\n<diff>\n{diff}\n</diff>",
                self.task
            )),
            timestamp: ctx.now(),
        })];
        let answer = ctx
            .ask(settings, &input)
            .await
            .map_err(PluginError::other)?;
        let message = answer.text();
        if message.trim().is_empty() {
            return Err(VcsError::EmptyDescription.into());
        }
        self.vcs
            .commit_all(message.trim(), self.bookmark.clone())
            .await?;
        Ok(())
    }
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
        let record = match self
            .vcs
            .end_turn(self.bookmark.clone(), self.since.clone())
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
        // The finish hook cannot change the run's terminal status. Keep
        // the failure durable and visible to the caller instead of treating
        // a failed commit as a successful handoff.
        if let Err(error) = self.commit_pending(ctx).await {
            let error = error.to_string();
            *self.finalization_error.lock().expect("not poisoned") =
                Some(error.clone());
            let _ = ctx
                .record(&serde_json::json!({
                    "workspace": self.name,
                    "finalization_error": error,
                }))
                .await;
        }
    }
}
