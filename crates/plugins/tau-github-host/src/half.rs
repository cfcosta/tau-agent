//! tau-github's host half (ADR 0030): it fills the `tau.forge` slot,
//! and signs in as the plugin's UI asks.

use std::sync::Arc;

use tau_agent::plugin::Plugin;
use tau_github::{Act, GitHubData, GitHubUi};
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

    /// How the sign-in stands.
    async fn data(&self, host: &GitHub, _cx: &HostCx) -> GitHubData {
        GitHubData {
            sign_in: host.sign_in_state().await,
        }
    }

    fn fill(&self, host: &GitHub, fills: &mut Fills) {
        fills.fill(&FORGE, Arc::new(host.clone()) as Arc<dyn Forge>);
    }

    /// Signs in or out ([`Act`]).
    async fn act(
        &self,
        host: &GitHub,
        action: serde_json::Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let act: Act = serde_json::from_value(action)?;
        host.act(act, cx).await
    }
}
