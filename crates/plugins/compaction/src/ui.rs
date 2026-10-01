//! Compaction's UI (ADR 0017): its entry on the Plugins screen and its
//! line in a run's plugin list. Its rewrites show as the run's, with
//! what they saved.

use gpui::App;
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    Handle,
    HostCx,
    Manifest,
    PluginInfo,
    PluginStatus,
    RunCtx,
    RunCx,
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

    fn host(&self, _cx: &HostCx) -> anyhow::Result<()> {
        Ok(())
    }

    /// Summarizing by the window of the run's own model.
    fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> Vec<Box<dyn Plugin>> {
        let mut compaction = Compaction::default();
        if let Some(model) = tau_ai::model::find(&run.model) {
            compaction = compaction.context_window(model.context_window);
        }
        vec![Box::new(compaction)]
    }

    fn catalog(&self, _host: &(), _cx: &HostCx, _settings: &()) -> PluginInfo {
        PluginInfo {
            name: NAME.into(),
            description: "Summarizes the context when it nears the window"
                .into(),
            seams: vec![Seam::Start, Seam::Rewrite],
            spend: 0.0,
            page: None,
        }
    }

    fn apply(&self, _state: &mut (), _body: &Value, _run: &mut dyn RunCx) {}

    fn new_ui(&self, _handle: Handle, _cx: &mut App) {}

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
