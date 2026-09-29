//! [`Delegate`]: the `delegate` tool, which hands a task to a sub-agent
//! that works on a copy of the caller's code and lands its changes on
//! the caller's stack when it returns (ADR 0009).
//!
//! 1. The caller's work so far is committed, and the sub-agent's
//!    workspace starts on that commit.
//! 2. The sub-agent runs as a child run of the caller, in a chat of its
//!    own, with its turns committed on its own stack.
//! 3. When it finishes, its changes land on the caller: the caller has
//!    not moved, so nothing is rewritten and nothing can conflict. The
//!    caller links them at the end of its turn, and the sub-agent
//!    closes: its workspace and bookmark go.
//! 4. When it fails, its changes are dropped and it closes the same way.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    tool::{AgentTool, ExecutionMode, ToolCtx, ToolOutput},
};

use crate::{
    Landing,
    project::Project,
    run_workspace::{Pending, RunWorkspace, bookmark},
    vcs::Identity,
};

/// Builds a sub-agent's agent around its workspace: the tools and
/// plugins it runs with, the workspace among them.
pub type ChildAgent =
    Arc<dyn Fn(RunWorkspace) -> anyhow::Result<Agent> + Send + Sync>;

/// The name the model calls the tool by.
pub const NAME: &str = "delegate";

/// Hands a task to a sub-agent working on the caller's code. Build one
/// per run, on that run's [`RunWorkspace`].
pub struct Delegate {
    parent: RunWorkspace,
    identity: Identity,
    child: ChildAgent,
    parameters: Value,
}

impl Delegate {
    pub fn new(
        parent: RunWorkspace,
        identity: Identity,
        child: impl Fn(RunWorkspace) -> anyhow::Result<Agent>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            parent,
            identity,
            child: Arc::new(child),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task, with everything the \
                            sub-agent needs to do it: it does not see this \
                            conversation.",
                    },
                },
                "required": ["task"],
                "additionalProperties": false,
            }),
        }
    }
}

/// Workspace names for sub-agents, unique in the process.
fn child_name(parent: &str) -> String {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    format!("{parent}-sub-{}", COUNT.fetch_add(1, Ordering::Relaxed))
}

/// Runs blocking project work off the async executor.
async fn blocking<T: Send + 'static>(
    project: &Project,
    work: impl FnOnce(&Project) -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let project = project.clone();
    tokio::task::spawn_blocking(move || work(&project)).await?
}

#[async_trait]
impl AgentTool for Delegate {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        "Hand a self-contained task to a sub-agent. It works in a chat of \
         its own on a copy of your code as it is now, your edits included. \
         When it finishes, its changes land on top of yours and its answer \
         comes back; if it fails, its changes are dropped. It does not see \
         this conversation, so give it everything it needs in `task`."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    // It moves the caller's working copy: nothing may edit files beside it.
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> anyhow::Result<ToolOutput> {
        let task = args["task"].as_str().unwrap_or_default().to_owned();
        let parent_bookmark = bookmark(&ctx.run);
        let project = self.parent.project().clone();

        // 1. The caller's work so far, as the sub-agent's base.
        let head = self
            .parent
            .vcs()
            .checkpoint(
                format!("tau: run {}, before delegating", ctx.run.0),
                &parent_bookmark,
            )
            .await?;
        if head.changed {
            self.parent.queue([Pending {
                commit_id: head.commit_id.clone(),
                change_id: head.change_id.clone(),
                from: None,
            }]);
        }
        let name = child_name(self.parent.name());
        let workspace =
            RunWorkspace::new(project.clone(), &name, self.identity.clone())?
                .with_base(head.commit_id.clone());

        // 2. The sub-agent, as a child run of the caller.
        let outcome = match (self.child)(workspace.clone()) {
            Ok(agent) => {
                agent
                    .as_tool(NAME, "")
                    .call(json!({ "input": task }), ctx)
                    .await
            }
            Err(error) => Err(error),
        };
        let child_bookmark = workspace.run().map(|run| bookmark(&run));

        let output = match outcome {
            // 3. Its changes land on the caller.
            Ok(output) => {
                let landing = match child_bookmark.clone() {
                    Some(name) => {
                        match blocking(&project, move |p| p.bookmark(&name)).await? {
                            Some(child_head) => Some(
                                self.parent
                                    .vcs()
                                    .land(child_head, &parent_bookmark, true)
                                    .await?,
                            ),
                            None => None,
                        }
                    }
                    None => None,
                };
                let landing = landing.unwrap_or_else(|| Landing {
                    changes: Vec::new(),
                    conflicts: Vec::new(),
                    head: head.commit_id.clone(),
                });
                let from = workspace.run().map(|run| run.0.to_string());
                self.parent.queue(landing.changes.iter().rev().map(|change| {
                    Pending {
                        commit_id: change.commit_id.clone(),
                        change_id: change.change_id.clone(),
                        from: from.clone(),
                    }
                }));
                let note = match landing.changes.len() {
                    0 => "[It changed no files.]".to_owned(),
                    1 => "[Its 1 change landed on top of yours.]".to_owned(),
                    n => format!("[Its {n} changes landed on top of yours.]"),
                };
                let text: String = output
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        tau_ai::message::InputBlock::Text(text) => {
                            Some(text.text.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                Ok(ToolOutput {
                    details: Some(json!({
                        "run": from,
                        "landing": landing,
                    })),
                    ..ToolOutput::text(format!("{text}\n\n{note}"))
                })
            }
            // 4. Or they are dropped.
            Err(error) => {
                let base = head.commit_id.clone();
                let workspace_name = name.clone();
                blocking(&project, move |p| {
                    if let Some(wc) = p.workspace_head(&workspace_name)? {
                        p.abandon_between(&base, &wc)?;
                    }
                    Ok(())
                })
                .await?;
                Err(error)
            }
        };

        // Closed, landed or dropped: its workspace and bookmark go.
        blocking(&project, move |p| {
            p.forget_workspace(&name)?;
            if let Some(name) = child_bookmark {
                p.remove_bookmark(&name)?;
            }
            Ok(())
        })
        .await?;
        output
    }
}
