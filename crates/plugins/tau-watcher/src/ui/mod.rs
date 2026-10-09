//! tau-watcher's UI (ADR 0017, 0032): a note under the step that asked
//! for it, the same note as a line above the composer while it is new,
//! and its settings: whether it is on, and the model it reads with.

pub mod settings;
pub mod view;

use tau_ui_plugin::{Manifest, UiPlugin, points};

use crate::{record::NAME, state::State};

/// tau-watcher with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct WatcherUi;

impl UiPlugin for WatcherUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = settings::Settings;
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::TRANSCRIPT, view::annotation)
            .contribute(points::COMPOSER_BAND, view::band)
            .status(State::status)
            .settings(settings::pane)
    }
}
