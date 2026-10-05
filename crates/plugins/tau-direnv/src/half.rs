//! tau-direnv's host half (ADR 0030).

use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_plugin::{HostCx, HostHalf, PluginInfo, RepoCtx, RunCtx, Seam};

use crate::{Act, DirenvUi, RepoData, Settings, host::Host};

/// tau-direnv on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirenvHost;

impl HostHalf for DirenvHost {
    type Plugin = DirenvUi;
    type Host = Host;

    /// The run hears of its workspace's environment. A sub-agent's
    /// commands go through it too, but no one sees the sub-agent's
    /// cards.
    async fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let dir = run.services.get::<tau_ui_plugin::WorkspaceDir>();
        match dir {
            Some(dir)
                if host.installed()
                    && run.kind != tau_ui_plugin::RunKind::SubAgent =>
            {
                Ok(vec![Box::new(host.plugin(&run.repo, dir.0.clone()))])
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn launcher(
        &self,
        host: &Host,
        repo: &RepoCtx,
        _settings: &Settings,
    ) -> Option<std::sync::Arc<dyn tau_agent::launch::Launcher>> {
        host.launcher(repo)
    }

    async fn catalog(
        &self,
        _host: &Host,
        _cx: &HostCx,
        settings: &Settings,
    ) -> PluginInfo {
        let on = settings.repos.values().filter(|on| **on).count();
        PluginInfo {
            group: tau_ui_plugin::Group::Environment,
            note: (on > 0).then(|| {
                tau_ui_plugin::Note::new(
                    format!("on in {on}"),
                    tau_ui_kit::theme::Tone::Quiet,
                )
            }),
            description: "Runs agent commands in the repository's direnv \
                          environment, once you allow it"
                .into(),
            seams: vec![Seam::Start],
            page: None,
            ..Default::default()
        }
    }

    async fn repo_data(
        &self,
        host: &Host,
        repo: &RepoCtx,
        _cx: &HostCx,
    ) -> RepoData {
        host.repo_data(repo)
    }

    async fn act(
        &self,
        host: &Host,
        action: Value,
        _cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let act: Act = serde_json::from_value(action)?;
        match act {
            Act::Decide { repo, load } => host.decide(&repo, load).await?,
            Act::Reload { run } => host.reload(&run)?,
        }
        Ok(None)
    }
}
