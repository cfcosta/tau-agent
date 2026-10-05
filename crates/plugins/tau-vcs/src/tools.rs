//! The version-control tools (`docs/reference/vcs.md`, "Tools"). Each
//! is a [`TypedTool`] on one [`Vcs`], run one call at a time.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tau_agent::{
    error::ToolError,
    tool::{
        AgentTool,
        ExecutionMode,
        ToolCtx,
        ToolOutput,
        TypedAdapter,
        TypedTool,
        typed,
    },
};

use crate::{
    ABORTED,
    error::VcsError,
    ops::{self, DEFAULT_LOG_LIMIT, Report},
    vcs::{Vcs, Worker},
};

/// Runs `op` on `vcs`'s thread and turns its report into the output.
async fn run(
    vcs: &Vcs,
    ctx: &ToolCtx,
    op: impl FnOnce(&mut Worker) -> Result<Report, VcsError> + Send + 'static,
) -> Result<ToolOutput, ToolError> {
    if ctx.cancel.is_cancelled() {
        return Err(ABORTED.into());
    }
    let report = vcs.call(op).await?;
    let mut output = ToolOutput::text(report.text);
    output.details = Some(report.details);
    Ok(output)
}

/// `vcs_status`'s arguments: none.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatusArgs {}

/// Shows the working copy's changes against its parent, and conflicts.
pub struct Status(pub Vcs);

#[async_trait]
impl TypedTool for Status {
    type Args = StatusArgs;
    const NAME: &'static str = crate::details::STATUS;
    const DESCRIPTION: &'static str = "Show the working-copy change (@): its change id, description and parent, the files it changes against the parent (A added, M modified, D deleted), and unresolved conflicts. File edits are recorded automatically; there is nothing to stage.";

    async fn call(
        &self,
        _args: StatusArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, ops::status).await
    }
}

/// `vcs_diff`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffArgs {
    /// Change id or commit id (or a unique prefix) to diff against its
    /// parent (default: the working copy)
    pub change: Option<String>,
    /// Only these files or directories, relative to the repository root
    /// (default: all)
    pub paths: Option<Vec<String>>,
}

/// Shows a change's diff against its parent, as unified diff text.
pub struct Diff(pub Vcs);

#[async_trait]
impl TypedTool for Diff {
    type Args = DiffArgs;
    const NAME: &'static str = crate::details::DIFF;
    const DESCRIPTION: &'static str = "Show a change's diff against its parent as a unified diff, the working copy by default. Output is truncated to 50KB; pass paths to narrow it.";

    async fn call(
        &self,
        args: DiffArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, move |worker| {
            ops::diff(worker, args.change, args.paths.unwrap_or_default())
        })
        .await
    }
}

/// `vcs_log`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LogArgs {
    /// Maximum number of changes to show (default: 10, at most 100)
    pub limit: Option<u32>,
}

/// Lists the working copy and its ancestors, newest first.
pub struct Log(pub Vcs);

#[async_trait]
impl TypedTool for Log {
    type Args = LogArgs;
    const NAME: &'static str = crate::details::LOG;
    const DESCRIPTION: &'static str = "List the working-copy change (@) and its ancestors, newest first. Each row: change id, commit id, flags (@, (empty), (conflict), (divergent), (immutable)), bookmarks in brackets, and the first line of the description. A divergent change id names more than one commit: pass its commit id instead. Pass these ids to the other vcs tools.";

    async fn call(
        &self,
        args: LogArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let limit = args.limit.unwrap_or(DEFAULT_LOG_LIMIT);
        run(&self.0, &ctx, move |worker| ops::log(worker, limit)).await
    }
}

/// `vcs_show`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowArgs {
    /// Change id or commit id (or a unique prefix) to show
    pub change: String,
}

/// Shows one change: its ids, author, parents, description and diff.
pub struct Show(pub Vcs);

#[async_trait]
impl TypedTool for Show {
    type Args = ShowArgs;
    const NAME: &'static str = crate::details::SHOW;
    const DESCRIPTION: &'static str = "Show one change: its ids, author, parents, full description and diff against its parent. Output is truncated to 50KB.";

    async fn call(
        &self,
        args: ShowArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, move |worker| ops::show(worker, args.change)).await
    }
}

/// `vcs_describe`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DescribeArgs {
    /// The new description; replaces the old one
    pub message: String,
}

/// Sets the working-copy change's description.
pub struct Describe(pub Vcs);

#[async_trait]
impl TypedTool for Describe {
    type Args = DescribeArgs;
    const NAME: &'static str = "vcs_describe";
    const DESCRIPTION: &'static str = "Set the description of the working-copy change (@), replacing the old one. Keeps working in the same change.";

    async fn call(
        &self,
        args: DescribeArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, move |worker| {
            ops::describe(worker, args.message)
        })
        .await
    }
}

/// `vcs_commit`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommitArgs {
    /// The description of the finished change
    pub message: String,
    /// Files or directories to commit, relative to the repository root
    /// (default: everything the working copy changes). The rest stays
    /// in the new working-copy change.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

/// Describes the working-copy change and starts a new one on top.
pub struct Commit(pub Vcs);

#[async_trait]
impl TypedTool for Commit {
    type Args = CommitArgs;
    const NAME: &'static str = "vcs_commit";
    const DESCRIPTION: &'static str = "Commit the working-copy change (@): set its description to message, then start a new change on top of it for further work. With paths, only those files or directories are committed, and the rest of the working copy's changes stay in the new change: commit unrelated edits apart, one change each. Nothing is committed for you: commit where a reviewer would want a boundary, with a Conventional Commits message (`type(scope): summary`, then a body saying what changed and why), and commit everything before you finish.";

    async fn call(
        &self,
        args: CommitArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, move |worker| {
            ops::commit(worker, args.message, args.paths)
        })
        .await
    }
}

/// `vcs_land`'s arguments: none.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LandArgs {}

/// Proposes landing the run's commits: on the run it was forked from, or
/// into trunk for a top-level run (ADR 0014). The person confirms;
/// nothing moves here.
///
/// A run that does not land declares it all the same, so its tools match
/// those of the runs that do and it can read their prompt cache: then
/// `refusal` says why, and every call fails with it.
pub struct Land {
    pub vcs: Vcs,
    pub refusal: Option<Arc<str>>,
}

#[async_trait]
impl TypedTool for Land {
    type Args = LandArgs;
    const NAME: &'static str = "vcs_land";
    const DESCRIPTION: &'static str = "Only for a chat the person forked or started on its own: when your work is finished and committed, propose landing it: your commits go onto the run you were forked from, or into main for a run started on its own. The person sees what would land and confirms; nothing moves until they do. Call it last, with everything committed. The main chat and sub-agents never call it: the main chat commits to main, and a sub-agent's commits land on its caller when it answers.";

    async fn call(
        &self,
        _args: LandArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        if ctx.cancel.is_cancelled() {
            return Err(ABORTED.into());
        }
        if let Some(refusal) = &self.refusal {
            return Err(refusal.to_string().into());
        }
        let working_copy = self.vcs.working_copy().await?;
        if !working_copy.paths.is_empty() {
            return Err(
                VcsError::Uncommitted(working_copy.paths.join(", ")).into()
            );
        }
        let mut output = ToolOutput::text(
            "Proposed landing your commits. The person sees what would land \
             and confirms it; you can stop here.",
        );
        output.details = Some(serde_json::json!({
            "proposed": true,
            "head": working_copy.head,
        }));
        Ok(output)
    }
}

/// `vcs_new`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NewArgs {
    /// Description for the new change (default: none)
    pub message: Option<String>,
}

/// Starts a new empty change on top of the working copy.
pub struct New(pub Vcs);

#[async_trait]
impl TypedTool for New {
    type Args = NewArgs;
    const NAME: &'static str = "vcs_new";
    const DESCRIPTION: &'static str = "Start a new empty change on top of the working-copy change (@), which keeps its description as is. Files do not change.";

    async fn call(
        &self,
        args: NewArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, move |worker| ops::new(worker, args.message)).await
    }
}

/// `vcs_restore`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreArgs {
    /// Files or directories to restore, relative to the repository root
    /// ("." for everything)
    pub paths: Vec<String>,
    /// Change id or commit id (or a unique prefix) to take the files from
    /// (default: the working copy's parent)
    pub from: Option<String>,
}

/// Restores paths in the working copy from another change.
pub struct Restore(pub Vcs);

#[async_trait]
impl TypedTool for Restore {
    type Args = RestoreArgs;
    const NAME: &'static str = "vcs_restore";
    const DESCRIPTION: &'static str = "Discard the working copy's changes to paths: make them match the parent change again (or the change in from). Files the parent lacks are deleted.";

    async fn call(
        &self,
        args: RestoreArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, move |worker| {
            ops::restore(worker, args.paths, args.from)
        })
        .await
    }
}

/// `vcs_resolve`'s arguments: explicitly selected files or directories.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveArgs {
    pub paths: Vec<String>,
}

/// Accepts the current clean materialization of markerless file conflicts.
pub struct Resolve(pub Vcs);

#[async_trait]
impl TypedTool for Resolve {
    type Args = ResolveArgs;
    const NAME: &'static str = "vcs_resolve";
    const DESCRIPTION: &'static str = "Explicitly accept the current file contents of markerless conflicts in paths. Unchanged files otherwise stay conflicted. Refuses files with conflict markers or non-file conflicts without resolving any selected path; edit markers or use vcs_restore to choose a committed side instead. Commit the resolution with vcs_commit.";

    async fn call(
        &self,
        args: ResolveArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, move |worker| {
            ops::resolve_conflicts(worker, args.paths)
        })
        .await
    }
}

/// `vcs_undo`'s arguments: none.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UndoArgs {}

/// Undoes the last operation these tools made in this workspace.
pub struct Undo(pub Vcs);

#[async_trait]
impl TypedTool for Undo {
    type Args = UndoArgs;
    const NAME: &'static str = "vcs_undo";
    const DESCRIPTION: &'static str = "Undo the last vcs_describe, vcs_commit, vcs_new, vcs_restore or vcs_resolve made in this workspace. Call it again to undo the one before. File edits made since are kept. Refuses when the last operation was not made by these tools.";

    async fn call(
        &self,
        _args: UndoArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        run(&self.0, &ctx, ops::undo).await
    }
}

/// A [`TypedTool`] whose batch runs one call at a time, so that, for
/// example, `vcs_commit` then `vcs_log` see each other's effects.
pub struct Sequential<T: TypedTool>(TypedAdapter<T>);

impl<T: TypedTool> Sequential<T> {
    pub fn new(tool: T) -> Self {
        Self(typed(tool))
    }
}

#[async_trait]
impl<T: TypedTool> AgentTool for Sequential<T> {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn description(&self) -> &str {
        self.0.description()
    }

    fn parameters(&self) -> &Value {
        self.0.parameters()
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        self.0.call(args, ctx).await
    }
}

/// Wraps `tool` for `Agent::tool`.
pub fn tool<T: TypedTool>(tool: T) -> Arc<dyn AgentTool> {
    Arc::new(Sequential::new(tool))
}
