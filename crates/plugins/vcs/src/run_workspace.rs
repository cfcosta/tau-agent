//! [`RunWorkspace`]: a run's own jj workspace in a [`Project`], with a
//! commit per turn (`docs/reference/vcs.md`, "Runs and turns").
//!
//! When the run starts, the plugin makes its workspace: on the project's
//! trunk for a new run, or on the commit of the turn a fork continues
//! from. After each turn it commits what the turn changed and stores a
//! [`Link`] from the turn to that commit. A fork of the run at a turn
//! inherits the links up to that turn, so it starts on that turn's code.
//! Whoever else cares what a turn changed, such as memory marking notes
//! about those files stale, hears of each commit through
//! [`RunWorkspace::on_commit`].

use std::{
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tau_agent::{
    event::RunEvent,
    plugin::{Plugin, PluginCtx, PluginRun, RunPlan},
    tool::RunId,
};

use crate::{
    TurnCommit,
    project::Project,
    vcs::{Identity, Vcs},
};

/// Hears of each turn's commit.
pub type CommitObserver = Arc<dyn Fn(&TurnCommit) + Send + Sync>;

/// The name the plugin stores its links under.
pub const PLUGIN: &str = "workspace";

/// The local bookmark on a run's newest commit: `tau/<run id>`.
pub fn bookmark(run: &RunId) -> String {
    format!("tau/{}", run.0)
}

/// A turn, and the commit that holds the files it left. Stored as the
/// plugin's record after each turn.
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
    observers: Vec<CommitObserver>,
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
    ) -> anyhow::Result<Self> {
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

    /// Calls `observer` with each turn's commit, after it is made.
    pub fn on_commit(
        mut self,
        observer: impl Fn(&TurnCommit) + Send + Sync + 'static,
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
    ) -> anyhow::Result<Box<dyn PluginRun>> {
        *self.run.lock().expect("not poisoned") = Some(ctx.run.clone());
        // A fork continues from the last turn it inherits.
        let inherited = plan.records().iter().rev().find_map(|record| {
            serde_json::from_value::<Link>(record.clone()).ok()
        });
        let project = self.project.clone();
        let name = self.name.clone();
        let base = self.base.clone();
        tokio::task::spawn_blocking(move || {
            // The turn's change may have been restacked since.
            let base = match (inherited, base) {
                (Some(link), _) => project.current([link])?.remove(0).commit_id,
                (None, Some(base)) => base,
                (None, None) => project.trunk()?,
            };
            project.add_workspace(&name, &base).map(|_| ())
        })
        .await??;
        Ok(Box::new(Turns {
            vcs: self.vcs.clone(),
            name: self.name.clone(),
            observers: self.observers.clone(),
            pending: self.pending.clone(),
        }))
    }
}

struct Turns {
    vcs: Vcs,
    name: String,
    observers: Vec<CommitObserver>,
    pending: Arc<Mutex<Vec<Pending>>>,
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
        // turn's own commit.
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
            };
            let _ = ctx
                .record(&serde_json::to_value(link).unwrap_or_default())
                .await;
        }
        let record = match self
            .vcs
            .checkpoint(
                format!("tau: run {} turn {turn}", ctx.run.0),
                bookmark(&ctx.run),
            )
            .await
        {
            Ok(commit) => {
                for observer in &self.observers {
                    observer(&commit);
                }
                serde_json::to_value(Link {
                    turn: *turn,
                    workspace: self.name.clone(),
                    commit_id: commit.commit_id,
                    change_id: commit.change_id,
                    changed: commit.changed,
                    from: None,
                })
                .unwrap_or_default()
            }
            // Not fatal to the run: a turn without a link cannot be
            // forked from, and the next turn's commit holds its files.
            Err(error) => serde_json::json!({
                "turn": turn,
                "error": error.to_string(),
            }),
        };
        let _ = ctx.record(&record).await;
    }
}
