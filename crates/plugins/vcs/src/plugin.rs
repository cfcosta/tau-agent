//! The version-control tools as a plugin (`docs/reference/plugins.md`),
//! for `Agent::plugin`.
//!
//! The plugin adds tools and nothing else: it keeps no state per run.
//! Every run of the agent works in the one workspace the [`Vcs`] opened.

use std::sync::Arc;

use tau_agent::{plugin::Plugin, tool::AgentTool};

use crate::{
    tools::{
        Commit,
        Describe,
        Diff,
        Log,
        New,
        Restore,
        Show,
        Status,
        Undo,
        tool,
    },
    vcs::Vcs,
};

/// The version-control tools on one workspace, as a plugin. All nine by
/// default; [`read_only`](Self::read_only) keeps the four that change
/// nothing but snapshots.
#[derive(Debug, Clone)]
pub struct VcsPlugin {
    vcs: Vcs,
    write: bool,
}

impl VcsPlugin {
    pub fn new(vcs: Vcs) -> Self {
        Self { vcs, write: true }
    }

    /// Keeps only `vcs_status`, `vcs_diff`, `vcs_log` and `vcs_show`.
    pub fn read_only(mut self) -> Self {
        self.write = false;
        self
    }

    pub fn vcs(&self) -> &Vcs {
        &self.vcs
    }
}

impl Plugin for VcsPlugin {
    fn name(&self) -> &str {
        "vcs"
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let vcs = &self.vcs;
        let mut tools = vec![
            tool(Status(vcs.clone())),
            tool(Diff(vcs.clone())),
            tool(Log(vcs.clone())),
            tool(Show(vcs.clone())),
        ];
        if self.write {
            tools.extend([
                tool(Describe(vcs.clone())),
                tool(Commit(vcs.clone())),
                tool(New(vcs.clone())),
                tool(Restore(vcs.clone())),
                tool(Undo(vcs.clone())),
            ]);
        }
        tools
    }
}
