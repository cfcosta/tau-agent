//! heiko-agents' settings pane (ADR 0029): an agent's model and effort,
//! the most it may use a day, and how long its events gather. Each
//! agent keeps them as its repository's copy, in heiko's data, where the
//! agent cannot change them. A view tree (ADR 0045).

use heiko_ui_plugin::{
    ViewCx,
    tree::{Ink, Kind, Node, Tone},
};
use serde_json::Value;

use super::{AgentsUi, SETTINGS, button};
use crate::Settings;

/// The efforts an agent can run at.
const EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// The tokens it may use a day, as the pane offers them.
const CAPS: [u64; 5] = [250_000, 500_000, 1_000_000, 2_000_000, 5_000_000];

/// How long its events gather, in seconds, as the pane offers it.
const WINDOWS: [(u64, &str); 4] =
    [(10, "10 s"), (30, "30 s"), (60, "1 min"), (300, "5 min")];

/// The pane; a choice saves the settings it holds, in the scope shown.
pub(super) fn render(view: &mut ViewCx<'_, AgentsUi>) -> Node {
    pane(view.settings)
}

/// The pane's tree: a row for each setting, its choices at the end, the
/// one chosen outlined.
pub fn pane(settings: &Settings) -> Node {
    let choice = |label: String, on: bool, next: Settings| {
        if on {
            return Node::Stamp {
                label,
                tone: Ink::Bright,
            };
        }
        button(
            &label,
            Kind::Secondary,
            SETTINGS,
            serde_json::to_value(next).unwrap_or(Value::Null),
        )
    };
    // A setting: its name over what it means, then its choices at the
    // row's end.
    let row = |name: &str, caption: &str, choices: Vec<Node>| {
        let mut row = vec![
            Node::stack([
                Node::Rich {
                    text: format!("**{name}**"),
                },
                Node::Toned {
                    text: caption.into(),
                    tone: Tone::Neutral,
                    mono: false,
                },
            ]),
            Node::Spacer,
        ];
        row.extend(choices);
        Node::Row { children: row }
    };
    let model = settings.model();
    let models = heiko_ai::model::plan_models()
        .into_iter()
        .map(|plan| {
            let id = plan.id.to_owned();
            choice(
                id.clone(),
                model == id,
                Settings {
                    model: Some(id),
                    ..settings.clone()
                },
            )
        })
        .collect();
    let efforts = EFFORTS
        .into_iter()
        .map(|effort| {
            choice(
                effort.to_owned(),
                settings.effort() == effort,
                Settings {
                    effort: Some(effort.to_owned()),
                    ..settings.clone()
                },
            )
        })
        .collect();
    let caps = CAPS
        .into_iter()
        .map(|cap| {
            choice(
                super::cap_label(cap),
                settings.daily_cap == cap,
                Settings {
                    daily_cap: cap,
                    ..settings.clone()
                },
            )
        })
        .collect();
    let windows = WINDOWS
        .into_iter()
        .map(|(seconds, label)| {
            choice(
                label.to_owned(),
                settings.window_secs == seconds,
                Settings {
                    window_secs: seconds,
                    ..settings.clone()
                },
            )
        })
        .collect();
    Node::Card {
        children: vec![
            row("Model", "What its turns run on.", models),
            Node::Rule,
            row("Effort", "How hard each turn thinks.", efforts),
            Node::Rule,
            row(
                "Most it may use a day",
                "At the cap it stops until tomorrow, or until you allow more.",
                caps,
            ),
            Node::Rule,
            row(
                "Gather events for",
                "An event waits this long for others, so a burst is one turn.",
                windows,
            ),
        ],
    }
}
