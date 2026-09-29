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

use std::{fmt, path::PathBuf, sync::Arc};

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
}

impl Link {
    /// Reads a link back from a stored record body.
    pub fn parse(body: &str) -> Option<Self> {
        serde_json::from_str(body).ok()
    }
}

/// A run's workspace, as a plugin. Build one per run, with a workspace
/// name of its own, and point the run's other tools at [`Self::dir`].
#[derive(Clone)]
pub struct RunWorkspace {
    project: Project,
    name: String,
    vcs: Vcs,
    observers: Vec<CommitObserver>,
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
        })
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
        _ctx: &PluginCtx,
    ) -> anyhow::Result<Box<dyn PluginRun>> {
        // A fork continues from the last turn it inherits.
        let inherited = plan.records().iter().rev().find_map(|record| {
            serde_json::from_value::<Link>(record.clone()).ok()
        });
        let project = self.project.clone();
        let name = self.name.clone();
        tokio::task::spawn_blocking(move || {
            let base = match inherited {
                Some(link) => link.commit_id,
                None => project.trunk()?,
            };
            project.add_workspace(&name, &base).map(|_| ())
        })
        .await??;
        Ok(Box::new(Turns {
            vcs: self.vcs.clone(),
            name: self.name.clone(),
            observers: self.observers.clone(),
        }))
    }
}

struct Turns {
    vcs: Vcs,
    name: String,
    observers: Vec<CommitObserver>,
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
