//! Compaction's UI (ADR 0017): its entry on the Plugins screen, its
//! line in a run's plugin list, and its switch for compacting idle
//! chats (ADR 0029). Its rewrites show as the run's, with what they
//! saved.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    HostCx,
    HostHalf,
    Manifest,
    PluginInfo,
    PluginStatus,
    RunCtx,
    Seam,
    UiPlugin,
    ViewCx,
    points::{self, AtRun},
    tree::Node,
};

use crate::{Compaction, NAME};

/// Compaction with its UI: what tau adds to an agent, after pruning.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompactionUi;

/// What the user set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether an idle chat is compacted before its prompt cache lapses
    /// (`docs/reference/compaction.md`, "Idle compaction"). On by
    /// default.
    pub idle: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { idle: true }
    }
}

impl UiPlugin for CompactionUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = Settings;

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::STATUS, |_: &AtRun, _| {
                Some(PluginStatus {
                    name: NAME.into(),
                    state: "watching the window".into(),
                    tone: Tone::Quiet,
                })
            })
            .settings(settings_pane)
    }

    /// The pane's switch flips idle compaction, in the scope it shows.
    fn on_action(
        &self,
        action: &str,
        _args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        if action == IDLE {
            let idle = !view.settings.idle;
            view.save_settings(&Settings { idle });
        }
    }
}

/// Its settings pane (ADR 0029): the switch for idle chats.
fn settings_pane(view: &mut ViewCx<'_, CompactionUi>) -> Node {
    let settings = *view.settings;
    Node::Card {
        children: vec![Node::Switch {
            label: "Compact idle chats".into(),
            detail: Some(
                "A long chat left idle is summarized shortly before its \
                 prompt cache lapses, while the summary still reads it from \
                 cache: your next message then starts from the summary \
                 instead of resending everything uncached."
                    .into(),
            ),
            on: settings.idle,
            action: IDLE.into(),
            args: Value::Null,
        }],
    }
}

/// The pane's one action: switch compacting idle chats.
const IDLE: &str = "idle";

/// tau-compaction on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompactionHost;

impl HostHalf for CompactionHost {
    type Plugin = CompactionUi;
    type Host = ();

    /// Summarizing by the window of the run's own model.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let mut compaction = Compaction::default().idle(settings.idle);
        if let Some(model) = tau_ai::model::find(&run.model) {
            compaction = compaction.context_window(model.context_window);
        }
        Ok(vec![Box::new(compaction)])
    }

    async fn catalog(
        &self,
        _host: &(),
        _cx: &HostCx,
        _settings: &Settings,
    ) -> PluginInfo {
        PluginInfo {
            group: tau_ui_plugin::Group::Context,
            description: "Summarizes the context when it nears the window, \
                          and an idle chat's before its cache lapses"
                .into(),
            seams: vec![Seam::Start, Seam::Rewrite],
            page: None,
            ..Default::default()
        }
    }
}
