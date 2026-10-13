//! tau-canvas's host half (ADR 0030): the tools for each run, and the
//! images the interface fetches to draw.

use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_plugin::{HostCx, HostHalf, PluginInfo, RunCtx, Seam, WorkspaceDir};

use crate::{
    CanvasUi,
    host::{CanvasPlugin, Storage},
    ui::{Action, Fetched},
};

/// tau-canvas on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct CanvasHost;

impl HostHalf for CanvasHost {
    type Plugin = CanvasUi;
    type Host = ();

    /// The canvas tools, reading files in the run's workspace and keeping
    /// images in the repository's canvas storage.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let dir = Storage::dir_of(&run.repo.dir);
        let storage =
            tokio::task::spawn_blocking(move || Storage::open(&dir)).await??;
        let root = run
            .services
            .get::<WorkspaceDir>()
            .map_or_else(|| run.repo.checkout.clone(), |dir| dir.0.clone());
        Ok(vec![Box::new(CanvasPlugin::new(storage, root))])
    }

    async fn catalog(
        &self,
        _host: &(),
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            description: "Shows you the images and documents the agent \
                          makes, in a pane beside the run"
                .into(),
            seams: vec![Seam::Tools],
            ..Default::default()
        }
    }

    /// Fetches an image's bytes for the interface to draw.
    async fn act(
        &self,
        _host: &(),
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let Action::Fetch { repo, blob } = serde_json::from_value(action)?;
        let id = blob.id.clone();
        let data = async {
            let repo = cx
                .repo(&repo)
                .ok_or_else(|| anyhow::anyhow!("no repository {repo}"))?;
            let dir = Storage::dir_of(&repo.dir);
            let storage =
                tokio::task::spawn_blocking(move || Storage::open(&dir))
                    .await??;
            storage.get(&blob).await
        }
        .await;
        let fetched = match data {
            Ok(data) => Fetched {
                id,
                data: Some(data),
                error: None,
            },
            Err(error) => Fetched {
                id,
                data: None,
                error: Some(format!("{error:#}")),
            },
        };
        Ok(Some(serde_json::to_value(fetched)?))
    }
}
