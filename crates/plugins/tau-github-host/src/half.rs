//! tau-github's host half (ADR 0030): it fills the `tau.forge` slot.

use std::sync::Arc;

use tau_agent::plugin::Plugin;
use tau_github::GitHubUi;
use tau_ui_plugin::{
    HostCx,
    HostHalf,
    PluginInfo,
    RunCtx,
    forge::{FORGE, Forge},
    slots::Fills,
};

use crate::GitHub;

/// tau-github on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitHubHost;

impl HostHalf for GitHubHost {
    type Plugin = GitHubUi;
    type Host = GitHub;

    /// None: runs reach GitHub only through the host's pushes and pull
    /// requests.
    async fn agent_plugins(
        &self,
        _host: &GitHub,
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        _host: &GitHub,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            description: "Clones, pushes and pull requests on GitHub".into(),
            ..Default::default()
        }
    }

    fn fill(&self, host: &GitHub, fills: &mut Fills) {
        fills.fill(&FORGE, Arc::new(host.clone()) as Arc<dyn Forge>);
    }
}
