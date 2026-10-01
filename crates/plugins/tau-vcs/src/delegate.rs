//! [`Delegate`]: the `delegate` tool, which hands a task to a sub-agent
//! that forks the caller's conversation, works on a copy of its code,
//! and lands its changes on the caller's stack when it returns (ADR
//! 0009, 0015).
//!
//! 1. The caller's work must be committed (ADR 0014): the tool refuses
//!    while `@` holds changes. The sub-agent's workspace starts on the
//!    caller's newest commit.
//! 2. The sub-agent runs as a child run of the caller, in a chat of its
//!    own. It forks the caller: it sees the conversation, the batch its
//!    call is in, and then its task. It commits its work on its own
//!    stack.
//! 3. Several calls in one batch run side by side, up to
//!    [`MAX_RUNNING`] at once, and apart from the batch's other tools.
//!    Each starts on the caller's head at the call.
//! 4. When one finishes, its changes land on the caller, one landing at
//!    a time, in the order they finish. The first cannot conflict; a
//!    later one that does lands its conflicts for the caller to
//!    resolve. The caller links them at the end of its turn, and the
//!    sub-agent closes: its workspace and bookmark go.
//!    One stopped by a limit lands too: its work was committed at its
//!    end, and the caller is told a limit cut it short.
//! 5. When it fails, or the caller is cancelled, its changes are
//!    dropped and it closes the same way.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, SubAgentError},
    error::ToolError,
    event::{LimitKind, StopReason},
    tool::{AgentTool, ExecutionMode, ToolCtx, ToolOutput},
};
use tau_ai::responses::request::ReasoningEffort;
use tokio::sync::{Mutex, Semaphore};

use crate::{
    Landing,
    details::DELEGATE,
    error::VcsError,
    project::Project,
    run_workspace::{Pending, RunWorkspace, bookmark},
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
pub type ChildAgent = Arc<
    dyn Fn(RunWorkspace, &ChildModel) -> Result<Agent, ToolError> + Send + Sync,
>;

/// Sub-agents of one caller that run at once. Calls past it wait for a
/// slot.
pub const MAX_RUNNING: usize = 4;

/// Hands a task to a sub-agent working on the caller's code. Build one
/// per run, on that run's [`RunWorkspace`].
pub struct Delegate {
    parent: RunWorkspace,
    identity: Identity,
    child: ChildAgent,
    parameters: Value,
    /// Slots for the sub-agents running at once.
    running: Arc<Semaphore>,
    /// Held while a sub-agent lands, so landings go one at a time.
    landing: Arc<Mutex<()>>,
}

impl Delegate {
    /// `models` are the ids a call may pick a model from.
    pub fn new(
        parent: RunWorkspace,
        identity: Identity,
        models: &[String],
        child: impl Fn(RunWorkspace, &ChildModel) -> Result<Agent, ToolError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let efforts: Vec<&str> = ReasoningEffort::ALL
            .iter()
            .map(|effort| effort.as_str())
            .collect();
        Self {
            parent,
            identity,
            child: Arc::new(child),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "What the sub-agent does. It sees \
                            this conversation, so name the task and what \
                            sets it apart from the others you delegate.",
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
            }),
            running: Arc::new(Semaphore::new(MAX_RUNNING)),
            landing: Arc::default(),
        }
    }
}

/// A workspace name for a sub-agent of `parent`: its caller's name and
/// twelve random hex digits, so no process takes over a workspace an
/// earlier one left behind.
fn child_name(parent: &str) -> String {
    // A v7 uuid ends in random bits; its start is the time.
    let id = uuid::Uuid::now_v7().simple().to_string();
    format!("{parent}-sub-{}", &id[20..])
}

/// Runs blocking project work off the async executor.
async fn blocking<T: Send + 'static>(
    project: &Project,
    work: impl FnOnce(&Project) -> Result<T, VcsError> + Send + 'static,
) -> Result<T, VcsError> {
    let project = project.clone();
    tokio::task::spawn_blocking(move || work(&project)).await?
}

/// A limit, as the result's details name it.
fn limit_name(limit: LimitKind) -> &'static str {
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
fn landing_note(
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

#[async_trait]
impl AgentTool for Delegate {
    fn name(&self) -> &str {
        DELEGATE
    }

    fn description(&self) -> &str {
        "Hand a task to a sub-agent. It forks this conversation, so it \
         knows what you know and `task` can be short. It works in a chat \
         of its own on a copy of your committed code: commit your work \
         with `vcs_commit` first. Call it several times in one turn to \
         run up to 4 sub-agents side by side, each on its own part. As \
         each finishes, its commits land on top of yours and its answer \
         comes back; changes that clash with an earlier one land as \
         conflicts for you to resolve. If it fails, its changes are \
         dropped. It runs on your model and effort unless `model` or \
         `effort` say otherwise; another model cannot reuse your prompt \
         cache, so it reads this whole conversation at full price."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    // Sub-agents run side by side; the caller's other tools stay out of
    // the way of the landings, which move its working copy.
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
        let parent_bookmark = self.parent.bookmark_of(&ctx.run);
        let project = self.parent.project().clone();

        // 1. The caller's committed work, as the sub-agent's base. The
        //    model makes the commits (ADR 0014): with work still in `@`,
        //    the sub-agent could not see it, so it commits first.
        let working_copy = self.parent.vcs().working_copy().await?;
        if !working_copy.paths.is_empty() {
            return Err(
                VcsError::Uncommitted(working_copy.paths.join(", ")).into()
            );
        }
        let head = working_copy.head;

        // 2. A slot, then the sub-agent, forking the caller.
        let slot = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                return Err("cancelled before it started".into());
            }
            slot = self.running.clone().acquire_owned() => {
                slot.expect("the semaphore is never closed")
            }
        };
        let name = child_name(self.parent.name());
        let workspace =
            RunWorkspace::new(project.clone(), &name, self.identity.clone())?
                .with_base(head.clone());
        let cancel = ctx.cancel.clone();
        let outcome = match (self.child)(workspace.clone(), &asked) {
            Ok(agent) => {
                agent
                    .as_tool(DELEGATE, "")
                    .forking()
                    .call(json!({ "input": task }), ctx)
                    .await
            }
            Err(error) => Err(error),
        };
        drop(slot);
        // Its answer, and the limit that cut it short, if one did. A
        // sub-agent at a limit committed its work at its end, as any run
        // does (ADR 0014), so it lands like one that finished.
        let outcome = match outcome {
            Ok(output) => Ok((output.text_content(), None)),
            Err(ToolError::SubAgent(SubAgentError::Stopped {
                stop: StopReason::Limit(limit),
                text,
                ..
            })) => Ok((text, Some(limit))),
            Err(error) => Err(error),
        };
        let child_bookmark = workspace.run().map(|run| bookmark(&run));

        // 3. Landings go one at a time, in the order sub-agents finish.
        //    Nothing lands once the caller is cancelled.
        let turn = self.landing.lock().await;
        let outcome = match outcome {
            Ok(_) if cancel.is_cancelled() => {
                Err("cancelled before it landed".into())
            }
            outcome => outcome,
        };
        let output = match outcome {
            // 4. Its changes land on the caller.
            Ok((text, limit)) => {
                // What the caller's head held in conflict before: the
                // note names only what this landing brought.
                let before = self.parent.vcs().working_copy().await?.head;
                let before =
                    blocking(&project, move |p| p.conflicts(&before)).await?;
                let landing = match child_bookmark.clone() {
                    Some(name) => {
                        match blocking(&project, move |p| p.bookmark(&name))
                            .await?
                        {
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
                    head: head.clone(),
                });
                let brought: Vec<String> = landing
                    .conflicts
                    .iter()
                    .filter(|path| !before.contains(path))
                    .cloned()
                    .collect();
                let from = workspace.run().map(|run| run.0.to_string());
                self.parent
                    .queue(landing.changes.iter().rev().map(|change| {
                        Pending {
                            commit_id: change.commit_id.clone(),
                            change_id: change.change_id.clone(),
                            from: from.clone(),
                        }
                    }));
                Ok(ToolOutput {
                    details: Some(json!({
                        "run": from,
                        "landing": landing,
                        "conflicts": brought,
                        "limit": limit.map(limit_name),
                    })),
                    ..ToolOutput::text(format!(
                        "{text}\n\n{}",
                        landing_note(&landing, &brought, limit)
                    ))
                })
            }
            // 5. Or they are dropped.
            Err(error) => {
                let base = head.clone();
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
        drop(turn);

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
