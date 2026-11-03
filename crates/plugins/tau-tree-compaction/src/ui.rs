//! Tree compaction's UI (ADR 0017): its switch in the settings, off by
//! default until the evaluation shows it helps, its entry on the
//! Plugins screen, and its line in a run's plugin list. Its rewrites
//! show as the run's, with what they saved.

use gpui::AnyElement;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    Fold,
    HostCx,
    HostHalf,
    Manifest,
    PluginInfo,
    PluginStatus,
    RunCtx,
    RunCx,
    Seam,
    UiPlugin,
    ViewCx,
    points::{self, AtRun},
    tree::Node,
};

use crate::{NAME, Record, TreeCompaction};

/// Tree compaction with its UI: what tau adds to an agent, after
/// pruning and before tau-compaction, which summarizes when it fails.
#[derive(Debug, Clone, Copy, Default)]
pub struct TreeCompactionUi;

/// Whether runs compact into a tree, as the user set it.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(default)]
pub struct Settings {
    /// Off by default: tau-compaction summarizes instead.
    pub on: bool,
}

/// What tree compaction did in a run, as its records leave it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Whether it was on as the run started; `None` before it said.
    pub on: Option<bool>,
    /// Compactions the run made.
    pub compactions: usize,
    /// Entries the history holds after the last one.
    pub entries: usize,
    /// Lines its view holds after the last one.
    pub lines: usize,
}

impl Fold for State {
    type Record = Record;

    fn apply(&mut self, record: Record, _run: &mut dyn RunCx) {
        match record {
            Record::Starting { on } => self.on = Some(on),
            Record::Compacted { entries, lines, .. } => {
                self.compactions += 1;
                self.entries = entries;
                self.lines = lines;
            }
            Record::Folded { .. } => {}
        }
    }
}

impl State {
    /// Its line in the run's plugin list.
    pub fn status(&self) -> String {
        match (self.on, self.compactions) {
            (Some(false), _) => "off".to_owned(),
            (_, 0) => "watching the window".to_owned(),
            (_, _) => {
                format!("{} messages in {} lines", self.entries, self.lines)
            }
        }
    }
}

impl UiPlugin for TreeCompactionUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = Settings;
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::STATUS, |_: &AtRun, view| {
                let state: State = view.state.cloned().unwrap_or_default();
                Some(PluginStatus {
                    name: NAME.into(),
                    state: state.status(),
                    tone: Tone::Quiet,
                })
            })
            .settings(settings_pane)
    }
}

/// Its settings pane (ADR 0029): the switch.
fn settings_pane(view: &mut ViewCx<'_, TreeCompactionUi>) -> AnyElement {
    let settings = *view.settings;
    let scope = view.scope().map(str::to_owned);
    let handle = view.handle.clone();
    let pane = Node::Card {
        children: vec![Node::Switch {
            label: "Compact into a tree".into(),
            detail: Some(
                "Near the window, older messages fold into one-line \
                 summaries the agent can zoom back into, instead of one \
                 summary. Each long message costs a request when it folds."
                    .into(),
            ),
            on: settings.on,
            action: ON.into(),
            args: Value::Null,
        }],
    };
    view.tree("tree-compaction-settings", &pane, move |action, _, cx| {
        if action == ON {
            let on = !settings.on;
            handle.save_settings_in(scope.as_deref(), &Settings { on }, cx);
        }
    })
}

/// The pane's one action: switch the tree on or off.
const ON: &str = "on";

/// Tree compaction on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct TreeCompactionHost;

impl HostHalf for TreeCompactionHost {
    type Plugin = TreeCompactionUi;
    type Host = ();

    /// When on, compacting by the window of the run's own model.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        if !settings.on {
            return Ok(Vec::new());
        }
        let mut compaction = TreeCompaction::default();
        if let Some(model) = tau_ai::model::find(&run.model) {
            compaction = compaction.context_window(model.context_window);
        }
        Ok(vec![Box::new(compaction)])
    }

    async fn starting(
        &self,
        _host: &(),
        _run: &RunCtx,
        settings: &Settings,
    ) -> Vec<Record> {
        vec![Record::Starting { on: settings.on }]
    }

    async fn catalog(
        &self,
        _host: &(),
        _cx: &HostCx,
        settings: &Settings,
    ) -> PluginInfo {
        let what = "Folds older context into one-line summaries the agent can zoom into";
        PluginInfo {
            group: tau_ui_plugin::Group::Context,
            description: if settings.on {
                what.into()
            } else {
                format!("{what} (off)")
            },
            seams: vec![Seam::Start, Seam::Rewrite],
            page: None,
            ..Default::default()
        }
    }
}
