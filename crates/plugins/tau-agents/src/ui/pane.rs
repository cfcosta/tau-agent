//! tau-agents' settings pane (ADR 0029): an agent's model and effort,
//! the most it may spend a day, and how long its events gather. Each
//! agent keeps them as its repository's copy, in tau's data, where the
//! agent cannot change them. A view tree (ADR 0045).

use serde_json::Value;
use tau_ui_plugin::{
    ViewCx,
    tree::{Kind, Node},
};

use super::{AgentsUi, SETTINGS, button};
use crate::Settings;

/// The efforts an agent can run at.
const EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// What it may spend a day, as the pane offers it.
const CAPS: [f64; 5] = [0.5, 1.0, 2.0, 5.0, 10.0];

/// How long its events gather, in seconds, as the pane offers it.
const WINDOWS: [(u64, &str); 4] =
    [(10, "10 s"), (30, "30 s"), (60, "1 min"), (300, "5 min")];

/// The pane; a choice saves the settings it holds, in the scope shown.
pub(super) fn render(view: &mut ViewCx<'_, AgentsUi>) -> Node {
    pane(view.settings)
}

/// The pane's tree: a row of choices for each setting, the one chosen
/// marked.
pub fn pane(settings: &Settings) -> Node {
    let choice = |label: String, on: bool, next: Settings| {
        button(
            &label,
            if on { Kind::Primary } else { Kind::Secondary },
            SETTINGS,
            serde_json::to_value(next).unwrap_or(Value::Null),
        )
    };
    let row = |name: &str, caption: &str, choices: Vec<Node>| {
        Node::stack([
            Node::Heading { text: name.into() },
            Node::text(caption),
            Node::Row { children: choices },
        ])
    };
    let model = settings.model();
    let models = tau_ai::model::plan_models()
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
                format!("${cap:.2}"),
                (settings.daily_cap - cap).abs() < f64::EPSILON,
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
            row(
                "Model",
                "What its turns run on. A small model keeps it cheap.",
                models,
            ),
            Node::Rule,
            row("Effort", "How hard each turn thinks.", efforts),
            Node::Rule,
            row(
                "Most it may spend a day",
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
