//! Desktop notices, as a plugin (ADR 0045): it fills the `tau.notify`
//! slot with the desktop's notification server, and its settings pane
//! switches notices off.

#[cfg(feature = "host")]
mod half;

#[cfg(feature = "host")]
pub use half::NotifyHost;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_plugin::{
    Declaration,
    Manifest,
    UiPlugin,
    ViewCx,
    notify::NOTIFY,
    tree::Node,
};

pub const NAME: &str = "tau-notify";

/// What the person set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether notices show; on until switched off.
    pub on: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { on: true }
    }
}

/// tau-notify with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct NotifyUi;

impl UiPlugin for NotifyUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = Settings;

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Declaration {
        Declaration::new().fills(&[NOTIFY.name()])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().settings(pane)
    }

    fn on_action(&self, action: &str, _: &Value, view: &mut ViewCx<'_, Self>) {
        if action == SWITCH {
            let next = Settings {
                on: !view.settings.on,
            };
            view.save_settings(&next);
        }
    }
}

/// The pane's switch.
pub const SWITCH: &str = "switch";

/// Its settings pane: the switch.
fn pane(view: &mut ViewCx<'_, NotifyUi>) -> Node {
    Node::Card {
        children: vec![Node::Switch {
            label: "Notify me while tau is in the background".into(),
            detail: Some(
                "A notice when a chat asks you something, is ready to land, \
                 failed or left conflicts on main, or something else waits \
                 on you. Clicking it opens the chat."
                    .into(),
            ),
            on: view.settings.on,
            action: SWITCH.into(),
            args: Value::Null,
        }],
    }
}
