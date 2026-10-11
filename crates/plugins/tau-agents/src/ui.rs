//! tau-agents' interface: what every window draws of the agents.

use tau_ui_plugin::{Manifest, UiPlugin};

use crate::{NAME, Overview, Settings};

/// tau-agents' interface half.
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentsUi;

impl UiPlugin for AgentsUi {
    type State = ();
    type Data = Overview;
    type RepoData = ();
    type Settings = Settings;
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
    }
}
