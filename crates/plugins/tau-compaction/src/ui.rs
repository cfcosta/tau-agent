//! Compaction's UI (ADR 0017): its entry on the Plugins screen and its
//! line in a run's plugin list. Its rewrites show as the run's, with
//! what they saved.

use tau_agent::plugin::Plugin;
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    HostCx,
    Manifest,
    PluginInfo,
    PluginStatus,
    RunCtx,
    Seam,
    UiPlugin,
    points::{self, AtRun},
};

use crate::{Compaction, NAME};

/// Compaction with its UI: what tau adds to an agent, after pruning.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompactionUi;

impl UiPlugin for CompactionUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Host = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    /// Summarizing by the window of the run's own model.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let mut compaction = Compaction::default();
        if let Some(model) = tau_ai::model::find(&run.model) {
            compaction = compaction.context_window(model.context_window);
        }
        Ok(vec![Box::new(compaction)])
    }

    async fn catalog(
        &self,
        _host: &(),
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            group: tau_ui_plugin::Group::Context,
            description: "Summarizes the context when it nears the window"
                .into(),
            seams: vec![Seam::Start, Seam::Rewrite],
            page: None,
            ..Default::default()
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::STATUS, |_: &AtRun, _| {
            Some(PluginStatus {
                name: NAME.into(),
                state: "watching the window".into(),
                tone: Tone::Quiet,
            })
        })
    }
}
