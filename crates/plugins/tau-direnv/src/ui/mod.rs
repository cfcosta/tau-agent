//! tau-direnv's UI (ADR 0017): the question in the composer's place
//! ([`points::COMPOSER`]), the loading and failure cards above the
//! transcript ([`points::RUN_BANNER`]), and the toggle on the
//! repository's menu ([`points::REPO_MENU`]).

pub mod view;

use std::time::Instant;

use serde::{Deserialize, Serialize};
use tau_ui_plugin::{Fold, Manifest, RunCx, UiPlugin, points};

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

/// The window's state: when a loading card last showed, so its clock
/// ticks while it does.
#[derive(Default)]
pub struct Ui {
    pub loading_shown: Option<Instant>,
    pub ticking: bool,
}

impl UiPlugin for DirenvUi {
    type State = State;
    type Data = ();
    type RepoData = RepoData;
    type Settings = Settings;
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::COMPOSER, view::question)
            .contribute(points::RUN_BANNER, view::card)
            .contribute(points::REPO_MENU, view::menu_entry)
            .settings(view::settings_pane)
    }
}
