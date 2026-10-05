//! tau-vcs's host half (ADR 0030).

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{HostCx, HostHalf, PluginInfo, RunCtx, Seam};
use tau_vcs::VcsUi;

/// tau-vcs on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct VcsHost;

impl HostHalf for VcsHost {
    type Plugin = VcsUi;
    type Host = ();

    /// None here: the host builds the tools with the run's workspace,
    /// which they act on.
    async fn agent_plugins(
        &self,
        _host: &(),
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        _host: &(),
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
}
