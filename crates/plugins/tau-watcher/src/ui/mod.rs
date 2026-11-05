//! tau-watcher's UI (ADR 0017, 0032): a note under the step that asked
//! for it, the same note as a line above the composer while it is new,
//! and its settings: whether it is on, and the model it reads with.

pub mod settings;
pub mod view;

use serde_json::Value;
use tau_ui_plugin::{Manifest, UiPlugin, ViewCx, points};

use crate::{record::NAME, state::State};

/// tau-watcher with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct WatcherUi;

impl UiPlugin for WatcherUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = settings::Settings;

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> tau_ui_plugin::Declaration {
        tau_ui_plugin::Declaration::new().seams(&[
            tau_ui_plugin::Seam::Start,
            tau_ui_plugin::Seam::BeforeRequest,
        ])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::TRANSCRIPT, view::annotation)
            .contribute(points::COMPOSER_BAND, view::band)
            .status(State::status)
            .settings(settings::pane)
    }

    /// A note's answer buttons, or its settings pane's controls.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        match action {
            view::ANSWER => view::on_action(args, view),
            _ => settings::on_action(action, args, view),
        }
    }
}
