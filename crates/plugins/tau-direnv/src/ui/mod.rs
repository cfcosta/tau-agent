//! tau-direnv's UI (ADR 0017): the question in the composer's place
//! ([`points::COMPOSER`]), the loading and failure cards above the
//! transcript ([`points::RUN_BANNER`]), and the toggle on the
//! repository's menu ([`points::REPO_MENU`]), each a view tree (ADR
//! 0045).

pub mod view;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_plugin::{Fold, Manifest, RunCx, UiPlugin, ViewCx, points};

use crate::{NAME, Record, RepoData, Settings};

/// tau-direnv with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirenvUi;

/// Where a run's workspace's environment stands, as tau-direnv's records
/// leave it: the last one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub now: Option<Record>,
}

impl Fold for State {
    type Record = Record;

    fn apply(&mut self, record: Record, _run: &mut dyn RunCx) {
        self.now = Some(record);
    }
}

impl UiPlugin for DirenvUi {
    type State = State;
    type Data = ();
    type RepoData = RepoData;
    type Settings = Settings;

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> tau_ui_plugin::Declaration {
        tau_ui_plugin::Declaration::new()
            .seams(&[tau_ui_plugin::Seam::Start, tau_ui_plugin::Seam::OnEvent])
            .services(&[tau_ui_plugin::services::names::WORKSPACE_DIR])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::COMPOSER, view::question)
            .contribute(points::RUN_BANNER, view::card)
            .contribute(points::REPO_MENU, view::menu_entry)
            .settings(view::settings_pane)
    }

    /// Every tree's buttons and switches carry the act they ask for.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        view::on_action(action, args, view);
    }
}
