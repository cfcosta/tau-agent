//! tau-watcher's settings (ADR 0029): whether it is on, and the model
//! its side requests go to.

use gpui::AnyElement;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ai::model::{PLAN_FAMILIES, family_version, plan_models};
use tau_ui_plugin::{
    ViewCx,
    tree::{Kind, Node},
};

use super::WatcherUi;

/// What the user set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Off until switched on: each note costs a request.
    pub enabled: bool,
    /// One of [`PLAN_FAMILIES`]: the newest model of it reads the run.
    /// None is the chat's own model.
    pub family: Option<String>,
}

impl Settings {
    /// The model that reads the run; none for each chat's own. A family
    /// the plan does not offer falls back on the chat's model.
    pub fn model(&self) -> Option<String> {
        let family = self.family.as_deref()?;
        plan_models()
            .into_iter()
            .find(|model| {
                family_version(&model.id)
                    .is_some_and(|(name, _)| name == family)
            })
            .map(|model| model.id.clone())
    }

    /// What the pane says the watcher reads with.
    pub fn runs_as(&self) -> String {
        match (&self.family, self.model()) {
            (_, Some(model)) => format!("Reads with {model}."),
            (None, None) => "Reads with each chat's model.".to_owned(),
            (Some(family), None) => format!(
                "No {family} model on the plan: reads with each chat's model."
            ),
        }
    }
}

/// Its settings pane: the switch, and the model.
pub fn pane(view: &mut ViewCx<'_, WatcherUi>) -> AnyElement {
    let settings = view.settings.clone();
    let scope = view.scope().map(str::to_owned);
    let handle = view.handle.clone();
    let models = std::iter::once((None, "Chat's"))
        .chain(PLAN_FAMILIES.iter().map(|f| (Some(*f), *f)))
        .map(|(family, label)| Node::Button {
            label: label.into(),
            action: MODEL.into(),
            args: family.map_or(Value::Null, Value::from),
            kind: if settings.family.as_deref() == family {
                Kind::Primary
            } else {
                Kind::Secondary
            },
            icon: None,
        });
    let pane = Node::stack([
        Node::Card {
            children: vec![
                Node::Switch {
                    label: "Note what I missed".into(),
                    detail: Some(
                        "Every 6th step, a side request reads the whole chat \
                         and almost always finds nothing. Now and then it \
                         offers one short note. Each check costs a request."
                            .into(),
                    ),
                    on: settings.enabled,
                    action: ENABLED.into(),
                    args: Value::Null,
                },
                Node::Rule,
                Node::stack([
                    Node::text("Model"),
                    Node::text(
                        "Reads the chat and writes the notes. Chat's uses \
                         whatever model the chat runs on.",
                    ),
                    Node::row(models),
                ]),
            ],
        },
        Node::Mono {
            text: settings.runs_as(),
        },
    ]);
    view.tree("watcher-settings", &pane, move |action, args, cx| {
        let next = match action {
            ENABLED => Settings {
                enabled: !settings.enabled,
                ..settings.clone()
            },
            MODEL => Settings {
                family: args.as_str().map(str::to_owned),
                ..settings.clone()
            },
            _ => return,
        };
        handle.save_settings_in(scope.as_deref(), &next, cx);
    })
}

/// The pane's actions: switch the watcher, and pick the family its
/// arguments name (none for the chat's own model).
const ENABLED: &str = "enabled";
const MODEL: &str = "model";
