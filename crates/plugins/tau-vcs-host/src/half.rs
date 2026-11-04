//! tau-vcs's host half (ADR 0030).

use std::sync::Arc;

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{
    HostCx,
    HostHalf,
    PluginInfo,
    RunCtx,
    Seam,
    slots::Fills,
    vcs::{self, VCS},
};
use tau_vcs::VcsUi;

use crate::JjVcs;

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
            seams: vec![Seam::Tools],
            page: None,
            ..Default::default()
        }
    }

    fn fill(&self, host: &JjVcs, fills: &mut Fills) {
        fills.fill(&VCS, Arc::new(host.clone()) as Arc<dyn vcs::Vcs>);
    }
}
