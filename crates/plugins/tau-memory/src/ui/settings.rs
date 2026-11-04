//! tau-memory's settings (ADR 0029): whether it saves what each run
//! taught as the run ends, the model it writes notes with then and
//! before compaction drops a conversation, and how hard that model
//! reasons. Search runs locally and needs none. Its pane is a view
//! tree (ADR 0045).

use serde::{Deserialize, Serialize};
use tau_ai::{
    model::{PLAN_FAMILIES, family_version, plan_models},
    responses::request::ReasoningEffort,
};
use tau_ui_plugin::{
    ViewCx,
    tree::{Kind, Node},
};

use super::{Do, MemoryUi, SETTINGS, button};

/// The family memory writes with unless the user picks another: the
/// newest of it on the plan.
pub const DEFAULT_FAMILY: &str = "luna";

/// The effort memory writes with unless the user picks another.
pub const DEFAULT_REASONING: &str = "low";

/// The efforts the pane offers; `None` leaves it to the model.
pub const EFFORTS: [Option<&str>; 4] =
    [None, Some("low"), Some("medium"), Some("high")];

/// What the model memory writes with is, as the user set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// One of [`PLAN_FAMILIES`]: the newest model of it writes.
    pub family: String,
    /// The effort, as [`ReasoningEffort::as_str`] spells it; `None`
    /// leaves it to the model.
    pub reasoning: Option<String>,
    /// Writes with the run's own model and effort instead.
    pub follow_chat: bool,
    /// Saves what a run taught when it ends, besides what the agent
    /// wrote and what compaction flushed. On by default: without it, a
    /// run that never compacts keeps only what the agent chose to write.
    pub after_each_run: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            family: DEFAULT_FAMILY.to_owned(),
            reasoning: Some(DEFAULT_REASONING.to_owned()),
            follow_chat: false,
            after_each_run: true,
        }
    }
}

impl Settings {
    /// The model memory writes with and its effort; `None` for the
    /// run's own. A family the plan does not offer falls back on the
    /// run's model, and an effort the model does not take on the
    /// model's default.
    pub fn model(&self) -> Option<(String, Option<ReasoningEffort>)> {
        if self.follow_chat {
            return None;
        }
        let model = plan_models().into_iter().find(|model| {
            family_version(&model.id)
                .is_some_and(|(family, _)| family == self.family)
        })?;
        let reasoning = self
            .reasoning
            .as_deref()
            .and_then(ReasoningEffort::parse)
            .filter(|effort| model.efforts.contains(effort));
        Some((model.id.clone(), reasoning))
    }

    /// What the pane says memory runs as: `gpt-6-luna · low`, or the
    /// chat's model.
    pub fn runs_as(&self) -> String {
        match self.model() {
            Some((model, effort)) => format!(
                "Runs as {model} · {}.",
                effort.map_or("auto", ReasoningEffort::as_str)
            ),
            None if self.follow_chat => "Runs as each chat's model.".to_owned(),
            None => format!(
                "No {} model on the plan: runs as each chat's model.",
                self.family
            ),
        }
    }
}

/// Its settings pane: the family, the effort, and the switch to write
/// with the chat's model instead. Its choices are saved in the scope the
/// pane shows.
pub fn pane(view: &mut ViewCx<'_, MemoryUi>) -> Node {
    pane_tree(view.settings)
}

/// The pane's tree for `settings`: each control saves the settings it
/// would make.
pub fn pane_tree(settings: &Settings) -> Node {
    let save = |next: Settings| Do::Save { settings: next };
    let switch =
        |label: &str, detail: &str, on: bool, next: Settings| Node::Switch {
            label: label.into(),
            detail: Some(detail.into()),
            on,
            action: SETTINGS.into(),
            args: save(next).args(),
        };
    // One choice of a row of them: primary when it is the setting.
    let choice = |label: &str, on: bool, next: Settings| {
        button(
            SETTINGS,
            label,
            if on { Kind::Primary } else { Kind::Secondary },
            &save(next),
        )
    };
    let families = PLAN_FAMILIES.iter().map(|family| {
        choice(
            family,
            settings.family == *family,
            Settings {
                family: (*family).to_owned(),
                ..settings.clone()
            },
        )
    });
    let efforts = EFFORTS.iter().map(|effort| {
        choice(
            effort.unwrap_or("auto"),
            settings.reasoning.as_deref() == *effort,
            Settings {
                reasoning: effort.map(str::to_owned),
                ..settings.clone()
            },
        )
    });
    let what = |name: &str, caption: &str| {
        Node::stack([Node::Heading { text: name.into() }, Node::text(caption)])
    };
    Node::stack([
        Node::Card {
            children: vec![
                switch(
                    "Save after each run",
                    "When a run ends, one request reads it and saves what a \
                     later run should know, often nothing. Off, memory keeps \
                     what the agent writes and what compaction is about to \
                     drop.",
                    settings.after_each_run,
                    Settings {
                        after_each_run: !settings.after_each_run,
                        ..settings.clone()
                    },
                ),
                Node::Rule,
                what(
                    "Model",
                    "Writes notes after each run and before compaction drops \
                     the conversation. Search runs locally and needs none.",
                ),
                Node::row(families),
                Node::Rule,
                what(
                    "Reasoning",
                    "How hard it thinks about what is worth keeping. Auto \
                     leaves it to the model.",
                ),
                Node::row(efforts),
                Node::Rule,
                switch(
                    "Use the chat's model instead",
                    "Notes come from whatever model and effort the run is on.",
                    settings.follow_chat,
                    Settings {
                        follow_chat: !settings.follow_chat,
                        ..settings.clone()
                    },
                ),
            ],
        },
        Node::Mono {
            text: settings.runs_as(),
        },
    ])
}
