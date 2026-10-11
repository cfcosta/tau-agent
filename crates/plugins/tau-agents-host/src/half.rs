//! tau-agents' host half (ADR 0030).

use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_agents::{Act, AgentsUi, Overview, Settings, Status};
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{CatalogEntry, HostCx, HostHalf, Note, PluginInfo, RunCtx};

use crate::Agents;

/// tau-agents on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct AgentsHost;

impl HostHalf for AgentsHost {
    type Plugin = AgentsUi;
    type Host = Agents;

    /// An agent's runs are its conversation's: what wakes it is the
    /// host's, not a plugin of the run.
    async fn agent_plugins(
        &self,
        _host: &Agents,
        _run: &RunCtx,
        _settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        host: &Agents,
        _cx: &HostCx,
        _settings: &Settings,
    ) -> PluginInfo {
        let overview = host.overview().await;
        let description = match overview.agents.len() {
            0 => "Agents, each a conversation of its own: none yet".to_owned(),
            1 => "Agents, each a conversation of its own: 1".to_owned(),
            n => format!("Agents, each a conversation of its own: {n}"),
        };
        PluginInfo {
            description,
            entries: overview.agents.iter().map(entry_row).collect(),
            ..Default::default()
        }
    }

    async fn data(&self, host: &Agents, _cx: &HostCx) -> Overview {
        host.overview().await
    }

    /// What the interface asks: an agent made, more spend allowed, an
    /// agent paused or resumed, a command sent to it. Making one answers
    /// `{ "repo": name }`.
    async fn act(
        &self,
        host: &Agents,
        action: Value,
        _cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        host.act(serde_json::from_value::<Act>(action)?).await
    }
}

/// An agent as a row of the Plugins screen, with a word when it needs
/// the person.
fn entry_row(agent: &tau_agents::Agent) -> CatalogEntry {
    let note = match &agent.status {
        Status::Broken { .. } => Some(Note::new("does not load", Tone::Danger)),
        Status::AtCap => Some(Note::new("at its cap", Tone::Warn)),
        Status::Paused => Some(Note::new("paused", Tone::Warn)),
        Status::Waiting | Status::Working => None,
    };
    CatalogEntry {
        name: agent.name.clone(),
        description: agent.purpose.clone(),
        note,
        ..Default::default()
    }
}
