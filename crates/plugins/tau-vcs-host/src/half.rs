//! tau-vcs's host half (ADR 0030).

use std::sync::Arc;

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{
    HostCx,
    HostHalf,
    PluginInfo,
    Prompt,
    RunCtx,
    RunKind,
    slots::Fills,
    vcs::{self, VCS},
};
use tau_vcs::VcsUi;

use crate::JjVcs;

/// What tau-vcs adds to every run's instructions: the model commits its
/// own work (ADR 0014).
const INSTRUCTIONS: &str = "Use the vcs tools for version control, never \
    `git` or `jj` in a shell command. Nothing is committed for you: your \
    commits are how your work is reviewed and landed, so commit with \
    `vcs_commit` wherever a reviewer would want a boundary, each with a \
    Conventional Commits message, and commit everything before you finish.";

/// tau-vcs on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct VcsHost;

impl HostHalf for VcsHost {
    type Plugin = VcsUi;
    type Host = JjVcs;

    /// None here: the host builds the tools with the run's workspace,
    /// which they act on.
    async fn agent_plugins(
        &self,
        _host: &JjVcs,
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        _host: &JjVcs,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            description: "status diff log show describe commit new restore \
                          resolve undo, on the run's workspace"
                .into(),
            page: None,
            ..Default::default()
        }
    }

    fn prompt(&self, _host: &JjVcs, _kind: RunKind, _settings: &()) -> Prompt {
        Prompt {
            instructions: Some(INSTRUCTIONS.to_owned()),
            context: None,
        }
    }

    fn fill(&self, host: &JjVcs, fills: &mut Fills) {
        fills.fill(&VCS, Arc::new(host.clone()) as Arc<dyn vcs::Vcs>);
    }
}
